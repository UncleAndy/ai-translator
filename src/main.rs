//! Command-line entry point of ai-translator.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use ai_translator::app::App;
use ai_translator::config::Config;
use ai_translator::source::audio::{CpalDeviceLister, render_devices};

#[derive(Parser)]
#[command(
    name = "ai-translator",
    version,
    about = "Синхронный переводчик речи и текста"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Проверить конфигурацию, не запуская перевод.
    Check {
        /// Путь к файлу конфигурации.
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Запустить перевод с микрофона.
    Run {
        /// Путь к файлу конфигурации.
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Прогнать заданный текст через шину, без провайдера.
    Script {
        /// Путь к файлу конфигурации.
        #[arg(short, long)]
        config: PathBuf,
        /// Текст для прогона.
        #[arg(short, long)]
        text: String,
    },
    /// Показать доступные устройства входа и выхода.
    ///
    /// Печатается тот же список, из которого выбирает сам переводчик: поле
    /// `input_device` в конфигурации задаётся одним из этих имён.
    Devices,
    /// Войти в аккаунт, который вещает в голосовой чат Telegram.
    ///
    /// Номер, код и пароль двухфакторной защиты вводятся здесь и никуда не
    /// записываются; остаётся только файл сессии из конфигурации.
    TelegramLogin {
        /// Путь к файлу конфигурации.
        #[arg(short, long)]
        config: PathBuf,
    },
}

fn main() -> ExitCode {
    init_tracing();

    // Ключи провайдера принято держать в .env рядом с конфигурацией.
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("не удалось запустить асинхронный runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(dispatch(cli))
}

/// Run the requested command.
async fn dispatch(cli: Cli) -> ExitCode {
    match cli.command {
        Command::Check { config } => check(&config),
        Command::Run { config } => run(&config).await,
        Command::Script { config, text } => script(&config, &text).await,
        Command::Devices => devices(),
        Command::TelegramLogin { config } => telegram_login(&config).await,
    }
}

/// Sign the broadcasting account in, once.
#[cfg(feature = "telegram-call")]
async fn telegram_login(path: &std::path::Path) -> ExitCode {
    use ai_translator::config::VoiceSinkConfig;
    use ai_translator::telegram_account::{Credentials, sign_in};

    let cfg = match load(path) {
        Ok(cfg) => cfg,
        Err(code) => return code,
    };

    let spec = cfg.sink.voice.iter().find_map(|sink| match sink {
        VoiceSinkConfig::TelegramCall {
            api_id_env,
            api_hash_env,
            session_file,
            ..
        } => Some((
            api_id_env.clone(),
            api_hash_env.clone(),
            session_file.clone(),
        )),
        VoiceSinkConfig::VirtualSink { .. } => None,
    });

    let Some((api_id_env, api_hash_env, session_file)) = spec else {
        eprintln!(
            "в конфигурации нет приёмника [[sink.voice]] type = \"telegram_call\": входить некуда"
        );
        return ExitCode::FAILURE;
    };

    let credentials = match Credentials::from_env(&api_id_env, &api_hash_env, &session_file) {
        Ok(credentials) => credentials,
        Err(reason) => {
            eprintln!("{reason}");
            return ExitCode::FAILURE;
        }
    };

    match sign_in(&credentials).await {
        Ok(account) => {
            println!("вход выполнен: {} (id {})", account.name, account.id);
            println!("сессия: {}", credentials.session_file.display());
            ExitCode::SUCCESS
        }
        Err(reason) => {
            eprintln!("{reason}");
            ExitCode::FAILURE
        }
    }
}

/// Sign the broadcasting account in, once: this build cannot broadcast at all.
#[cfg(not(feature = "telegram-call"))]
async fn telegram_login(_path: &std::path::Path) -> ExitCode {
    eprintln!(
        "эта сборка не умеет транслировать в голосовой чат: платформа или архитектура не \
         поддерживается"
    );
    ExitCode::FAILURE
}

/// Check the configuration without starting anything.
fn check(path: &std::path::Path) -> ExitCode {
    let cfg = match load(path) {
        Ok(cfg) => cfg,
        Err(code) => return code,
    };

    for warning in cfg.warnings() {
        eprintln!("предупреждение: {warning}");
    }

    let problems = cfg.validate();

    if problems.is_empty() {
        println!("конфигурация корректна: {}", path.display());
        return ExitCode::SUCCESS;
    }

    eprintln!("конфигурация не готова к запуску: {}", path.display());
    for problem in &problems {
        eprintln!("  - {problem}");
    }

    ExitCode::FAILURE
}

/// Translate from the microphone until interrupted.
async fn run(path: &std::path::Path) -> ExitCode {
    let cfg = match load(path) {
        Ok(cfg) => cfg,
        Err(code) => return code,
    };

    for warning in cfg.warnings() {
        eprintln!("предупреждение: {warning}");
    }

    let mut app = match App::build(&cfg) {
        Ok(app) => app,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    println!("приёмники: {}", app.sink_ids().join(", "));

    if let Err(error) = app.start_audio(&cfg).await {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }

    println!("перевод идёт; Ctrl+C — остановка");
    wait_for_stop().await;

    if let Err(error) = app.shutdown().await {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }

    println!("остановлено");
    ExitCode::SUCCESS
}

/// Feed a text through the bus without the provider.
async fn script(path: &std::path::Path, text: &str) -> ExitCode {
    let cfg = match load(path) {
        Ok(cfg) => cfg,
        Err(code) => return code,
    };

    let app = match App::build(&cfg) {
        Ok(app) => app,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    println!("приёмники: {}", app.sink_ids().join(", "));

    // Прерывание прогона останавливает публикацию, но не отменяет остановку:
    // уже поданный текст должен дойти до приёмников и попасть в файл.
    tokio::select! {
        published = app.publish_text(&cfg, text) => {
            println!("опубликовано событий: {published}");
        }
        () = wait_for_stop() => {
            println!("прогон прерван");
        }
    }

    if let Err(error) = app.shutdown().await {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

/// Print the audio devices the translator can choose from.
///
/// The names are the ones the audio source itself matches against, so an
/// `input_device` taken from this list is accepted as is.
fn devices() -> ExitCode {
    print!("{}", render_devices(&CpalDeviceLister::new()));

    ExitCode::SUCCESS
}

/// Read a configuration file, reporting the reason on stderr.
fn load(path: &std::path::Path) -> Result<Config, ExitCode> {
    Config::load(path).map_err(|error| {
        eprintln!("конфигурация не прочитана ({}): {error}", path.display());
        ExitCode::FAILURE
    })
}

/// Wait for a stop signal: Ctrl+C or SIGTERM.
///
/// A second Ctrl+C ends the process at once: if the shutdown drags on (an
/// external service is not answering), waiting for it forever is not an option.
async fn wait_for_stop() {
    if stop_signal().await.is_err() {
        return;
    }

    println!("\nостанавливаюсь; ещё раз Ctrl+C — выйти сразу");

    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("немедленный выход");
            std::process::exit(130);
        }
    });
}

/// Resolve when the process is asked to stop.
///
/// SIGTERM is the standard stop signal of a service, so it must flush the
/// translation the same way Ctrl+C does. The handlers are released when this
/// future ends, so a repeated SIGTERM still kills a stuck process.
async fn stop_signal() -> std::io::Result<()> {
    // SIGTERM exists on Unix only: on Windows the console handler behind
    // `ctrl_c` is the whole story, so a service there stops with Ctrl+C or
    // with the window being closed.
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}

/// Log to stderr, leaving stdout to the translation itself.
fn init_tracing() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .init();
}
