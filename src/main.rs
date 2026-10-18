//! Command-line entry point of ai-translator.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use ai_translator::app::App;
use ai_translator::config::Config;

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
    }
}

/// Check the configuration without starting anything.
fn check(path: &std::path::Path) -> ExitCode {
    let cfg = match load(path) {
        Ok(cfg) => cfg,
        Err(code) => return code,
    };

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

    let published = app.publish_text(&cfg, text).await;
    println!("опубликовано событий: {published}");

    if let Err(error) = app.shutdown().await {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

/// Read a configuration file, reporting the reason on stderr.
fn load(path: &std::path::Path) -> Result<Config, ExitCode> {
    Config::load(path).map_err(|error| {
        eprintln!("конфигурация не прочитана ({}): {error}", path.display());
        ExitCode::FAILURE
    })
}

/// Wait for Ctrl+C; a second signal ends the process at once.
async fn wait_for_stop() {
    if tokio::signal::ctrl_c().await.is_err() {
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

/// Log to stderr, leaving stdout to the translation itself.
fn init_tracing() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .init();
}
