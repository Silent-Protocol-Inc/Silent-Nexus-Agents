//! `snx` — the NEXUS command-line entry point.
//!
//! Local intelligence. Controlled execution.

mod approval;
mod cli;
mod commands;
mod ui;

use clap::{CommandFactory, Parser};
use cli::{CatalogCmd, Cli, Command, ConfigCmd};
use nexus_app::App;
use std::process::ExitCode;
use std::sync::Arc;

#[tokio::main]
async fn main() -> ExitCode {
    let args = Cli::parse();
    let color = !args.no_color && std::env::var_os("NO_COLOR").is_none();

    match dispatch(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if color {
                eprintln!("\x1b[38;5;203m✗\x1b[0m {e}");
            } else {
                eprintln!("error: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

fn init_logging(verbose: bool) -> Option<nexus_observability::LogGuard> {
    // Best-effort: resolve the workspace state dir for the log file.
    let dir = std::env::current_dir()
        .ok()
        .and_then(|w| nexus_core::config::ConfigPaths::discover(&w).ok())
        .map(|p| p.state_dir.join("logs"));
    if let Some(directory) = dir.as_deref() {
        let _ = nexus_core::permissions::repair_private_tree(directory);
    }
    nexus_observability::init_tracing(dir.as_deref(), verbose).ok()
}

async fn dispatch(args: Cli) -> anyhow::Result<()> {
    let json = args.json;
    let no_color = args.no_color;
    let inline = args.inline;

    // Completion generation needs no app context.
    if let Some(Command::Completion { shell }) = &args.command {
        let mut cmd = Cli::command();
        let name = cmd.get_name().to_string();
        clap_complete::generate(*shell, &mut cmd, name, &mut std::io::stdout());
        return Ok(());
    }

    // Schema inspection must remain available even when an existing config is
    // invalid and must not create or repair workspace state.
    if matches!(&args.command, Some(Command::Config(ConfigCmd::Schema))) {
        println!(
            "{}",
            serde_json::to_string_pretty(&nexus_core::config::Config::json_schema())?
        );
        return Ok(());
    }

    // Brand/version output has no runtime or configuration dependency.
    if let Some(Command::About(a)) = args.command {
        return commands::about(a, no_color);
    }

    // Structured logs go to the state dir; never to stdout (which carries
    // command output and, in MCP serve mode, the JSON-RPC stream). Initialize
    // only after the side-effect-free completion/schema/about paths.
    let _log_guard = init_logging(args.verbose);

    // Onboarding runs before any config exists, so handle it before bootstrap.
    if let Some(Command::Setup(a)) = args.command {
        return commands::setup(a, no_color).await;
    }

    // Default (no subcommand) and `chat` launch the interactive TUI.
    let command = args.command.unwrap_or(Command::Chat);

    let app = App::bootstrap(no_color).await?;

    match command {
        Command::Chat => {
            use std::io::IsTerminal;
            if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
                anyhow::bail!(
                    "the interactive TUI needs a terminal — use `snx run \"<objective>\"` for non-interactive use"
                );
            }
            // First run (no models) still opens the TUI — it greets the
            // operator and walks them through /setup interactively.
            if inline {
                nexus_tui::run_inline(Arc::new(app)).await?;
            } else {
                nexus_tui::run(Arc::new(app)).await?;
            }
            Ok(())
        }
        Command::Run(a) => commands::run(&app, a, json).await,
        Command::Status => commands::status(&app, json).await,
        Command::Usage => {
            let report = nexus_app::services::usage_report(&app).await?;
            commands::render(&app, &report);
            Ok(())
        }
        Command::Model(c) => commands::model(&app, c, json).await,
        Command::Resume { id: Some(id) } if !json => {
            use std::io::IsTerminal;
            if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
                anyhow::bail!(
                    "`snx resume <id>` launches the TUI and needs a terminal; \
                     use `snx run --session <id> \"<objective>\"` non-interactively"
                );
            }
            if app.sessions().get(&id).is_err() && app.goals().get(&id).is_err() {
                anyhow::bail!("`{id}` is neither a session nor a goal id");
            }
            if inline {
                nexus_tui::run_resume_inline(Arc::new(app), id).await?;
            } else {
                nexus_tui::run_resume(Arc::new(app), id).await?;
            }
            Ok(())
        }
        Command::Resume { id } => commands::resume(&app, id, json).await,
        Command::Continue { id } => commands::continue_session(&app, id, json).await,
        Command::Summary { session } => commands::summary(&app, session, json).await,
        Command::Test { command } => commands::test(&app, command).await,
        Command::Branch(command) => commands::branch(&app, command).await,
        Command::Commit(args) => commands::commit(&app, args).await,
        Command::Theme { name } => commands::theme(&app, name).await,
        Command::Goal(c) => commands::goal(&app, c, json).await,
        Command::Session(c) => commands::session(&app, c, json).await,
        Command::Persona(c) => commands::persona(&app, c, json).await,
        Command::Profile(c) => commands::profile(&app, c, json).await,
        Command::Memory(c) => commands::memory(&app, c, json).await,
        Command::Skill(c) => commands::skill(&app, c, json).await,
        Command::Mcp(c) => commands::mcp(&app, c, json).await,
        Command::Connector(c) => commands::connector(&app, c, json).await,
        Command::Sandbox(c) => commands::sandbox(&app, c, json).await,
        Command::Index(c) => commands::index(&app, c, json).await,
        Command::Tools(c) => commands::tools(&app, c, json).await,
        Command::Catalog(c) => {
            commands::catalog(&app, c.command.unwrap_or(CatalogCmd::List), json).await
        }
        Command::Auth(c) => commands::auth(&app, c, json).await,
        Command::Config(c) => commands::config(&app, c).await,
        Command::Audit(a) => commands::audit(&app, a, json).await,
        Command::Logs => commands::logs(&app).await,
        Command::Init => commands::init(&app).await,
        Command::Doctor(args) => commands::doctor(&app, args, json).await,
        Command::Maintenance(command) => commands::maintenance(&app, command, json).await,
        Command::Worker { idle_secs } => {
            nexus_app::worker::run(Arc::new(app), idle_secs).await?;
            Ok(())
        }
        Command::About(_) | Command::Setup(_) | Command::Completion { .. } => {
            unreachable!("handled above")
        }
    }
}
