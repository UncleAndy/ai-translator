//! Sign-in of the account that broadcasts into a Telegram voice chat.
//!
//! A group call is MTProto, and Telegram does not let bots into calls, so the
//! broadcast belongs to a real account. That account signs in once, here: the
//! phone number, the code from Telegram and the two-factor password, if the
//! account has one. What is kept afterwards is the session file — everything
//! the program needs to act as the account, and therefore a secret: it is
//! created readable by the owner only.
//!
//! Nothing of the sign-in goes into the configuration: the file names the
//! session, the environment names the application.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use grammers_client::{Client, SignInError};
use grammers_mtsender::SenderPool;
use grammers_session::storages::SqliteSession;

/// What the account needs to sign in.
#[derive(Debug, Clone)]
pub struct Credentials {
    /// `api_id` of the application from my.telegram.org.
    pub api_id: i32,
    /// `api_hash` of the same application.
    pub api_hash: String,
    /// File holding the session.
    pub session_file: PathBuf,
}

impl Credentials {
    /// Read the credentials from the environment names the configuration gives.
    ///
    /// # Errors
    ///
    /// Returns a readable reason when a variable is missing or the `api_id` is
    /// not a number.
    pub fn from_env(
        api_id_env: &str,
        api_hash_env: &str,
        session_file: &str,
    ) -> Result<Self, String> {
        let api_id = std::env::var(api_id_env)
            .map_err(|_| format!("не задана переменная окружения {api_id_env}"))?;
        let api_hash = std::env::var(api_hash_env)
            .map_err(|_| format!("не задана переменная окружения {api_hash_env}"))?;

        let api_id: i32 = api_id
            .trim()
            .parse()
            .map_err(|_| format!("{api_id_env} должна быть числом, а не «{api_id}»"))?;

        if api_hash.trim().is_empty() {
            return Err(format!("{api_hash_env} пуста"));
        }

        Ok(Self {
            api_id,
            api_hash: api_hash.trim().to_string(),
            session_file: expand_home(session_file),
        })
    }
}

/// Turn a leading `~` into the home directory the shell would use.
fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }

    PathBuf::from(path)
}

/// Who is signed in, as the account shows itself.
#[derive(Debug, Clone)]
pub struct Account {
    /// Numeric identifier.
    pub id: i64,
    /// Display name as Telegram returns it.
    pub name: String,
}

/// Sign the account in, unless the session already holds a signed-in one.
///
/// # Errors
///
/// Returns a readable reason: the session file cannot be used, Telegram refused
/// the number or the code, or the answer of the API is an error.
pub async fn sign_in(credentials: &Credentials) -> Result<Account, String> {
    prepare_session_dir(&credentials.session_file)?;

    let session = Arc::new(
        SqliteSession::open(&credentials.session_file)
            .await
            .map_err(|e| {
                format!(
                    "файл сессии не открывается ({}): {e}",
                    credentials.session_file.display()
                )
            })?,
    );

    let SenderPool { runner, handle, .. } =
        SenderPool::new(Arc::clone(&session), credentials.api_id);
    let client = Client::new(handle);
    let pool = tokio::spawn(runner.run());

    let result = authenticate(&client, credentials).await;

    // The session is written on the way out, so the pool is closed before the
    // process returns instead of being left to the runtime shutdown.
    drop(client);
    let _ = pool.await;

    restrict_session_file(&credentials.session_file);

    result
}

/// Ask the account who it is: a saved session answers without any prompt.
async fn authenticate(client: &Client, credentials: &Credentials) -> Result<Account, String> {
    if client
        .is_authorized()
        .await
        .map_err(|e| format!("Telegram недоступен: {e}"))?
    {
        return current_account(client).await;
    }

    let phone = prompt("номер телефона (например +79990000000): ")?;
    if phone.trim().is_empty() {
        return Err("номер телефона не введён".into());
    }

    let token = client
        .request_login_code(phone.trim(), &credentials.api_hash)
        .await
        .map_err(|e| format!("код не запрошен: {e}"))?;

    println!("код отправлен в Telegram — введите его здесь");

    let code = prompt("код из сообщения: ")?;
    if code.trim().is_empty() {
        return Err("код не введён".into());
    }

    match client.sign_in(&token, code.trim()).await {
        Ok(_) => current_account(client).await,
        Err(SignInError::PasswordRequired(password_token)) => {
            // The password is typed without echo: it never shows on the screen
            // and never reaches the command history.
            let password = rpassword::prompt_password("пароль двухфакторной защиты: ")
                .map_err(|e| format!("пароль не прочитан: {e}"))?;

            client
                .check_password(password_token, password.trim())
                .await
                .map_err(|e| format!("пароль не принят: {e}"))?;

            current_account(client).await
        }
        Err(SignInError::InvalidCode) => Err("код не подошёл".into()),
        Err(SignInError::InvalidPassword(_)) => {
            Err("пароль двухфакторной защиты не подошёл".into())
        }
        Err(SignInError::SignUpRequired) => {
            Err("у этого номера нет аккаунта Telegram: вход невозможен".into())
        }
        Err(SignInError::Other(e)) => Err(format!("вход не выполнен: {e}")),
    }
}

/// Read who is signed in.
async fn current_account(client: &Client) -> Result<Account, String> {
    let user = client
        .get_me()
        .await
        .map_err(|e| format!("аккаунт не прочитан: {e}"))?;

    let name = user
        .first_name()
        .map(str::to_string)
        .or_else(|| user.last_name().map(str::to_string))
        .unwrap_or_else(|| "без имени".to_string());

    let id = user
        .id()
        .bare_id()
        .ok_or_else(|| "аккаунт не сообщил свой идентификатор".to_string())?;

    Ok(Account { id, name })
}

/// Create the directory of the session with owner-only access.
fn prepare_session_dir(path: &Path) -> Result<(), String> {
    let Some(dir) = path.parent() else {
        return Ok(());
    };

    if dir.as_os_str().is_empty() {
        return Ok(());
    }

    std::fs::create_dir_all(dir)
        .map_err(|e| format!("каталог сессии не создан ({}): {e}", dir.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }

    Ok(())
}

/// Keep the session readable by its owner only: it is the account itself.
fn restrict_session_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The database keeps its own side files next to the chosen name.
        for candidate in [
            path.to_path_buf(),
            path.with_extension("session-wal"),
            path.with_extension("session-shm"),
        ] {
            if candidate.exists() {
                let _ = std::fs::set_permissions(candidate, std::fs::Permissions::from_mode(0o600));
            }
        }
    }

    #[cfg(not(unix))]
    let _ = path;
}

/// Ask the user for a line of text.
fn prompt(question: &str) -> Result<String, String> {
    print!("{question}");
    io::stdout()
        .flush()
        .map_err(|e| format!("вопрос не выведен: {e}"))?;

    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("ответ не прочитан: {e}"))?;

    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_variable_is_named() {
        let err = Credentials::from_env(
            "AI_TRANSLATOR_NO_SUCH_ID",
            "AI_TRANSLATOR_NO_SUCH_HASH",
            "x",
        )
        .unwrap_err();
        assert!(err.contains("AI_TRANSLATOR_NO_SUCH_ID"), "{err}");
    }

    #[test]
    fn a_non_numeric_api_id_is_reported() {
        // SAFETY: the test sets and reads its own variable.
        unsafe { std::env::set_var("AI_TRANSLATOR_TEST_API_ID", "не число") };
        unsafe { std::env::set_var("AI_TRANSLATOR_TEST_API_HASH", "abc") };

        let err = Credentials::from_env(
            "AI_TRANSLATOR_TEST_API_ID",
            "AI_TRANSLATOR_TEST_API_HASH",
            "x",
        )
        .unwrap_err();

        assert!(err.contains("должна быть числом"), "{err}");
    }

    #[test]
    fn a_home_relative_session_is_expanded() {
        let path = expand_home("~/state/session");
        assert!(!path.to_string_lossy().starts_with('~'), "{path:?}");
        assert!(path.ends_with("state/session"), "{path:?}");
    }
}
