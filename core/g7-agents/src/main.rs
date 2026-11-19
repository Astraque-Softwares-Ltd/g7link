// SPDX-License-Identifier: MPL-2.0
//! `g7-agents`: the agent host, its approval bridge, and a small command
//! line for listing agents, folders and threads.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use g7_agents::api::{AgentInfo, AuthKind, ClientKind};
use g7_agents::client::{self, BlockingClient, ClientError};
use g7_agents::host;

/// The host log is rotated past this size; one old file is kept.
const LOG_ROTATE_BYTES: u64 = 1024 * 1024;

#[derive(Parser)]
#[command(
    name = "g7-agents",
    version,
    about = "G7 Agents: runs your own coding-agent CLIs for G7 Linux and G7 Link"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the host (started on demand by the launcher and G7 Link).
    Serve,
    /// The approval bridge Claude starts; not for people.
    #[command(hide = true)]
    McpApprove,
    /// Show which agents are installed and signed in.
    Agents {
        /// Check again instead of using the last result.
        #[arg(long)]
        refresh: bool,
    },
    /// The folders agents may work in.
    Folders {
        #[command(subcommand)]
        command: FoldersCommand,
    },
    /// List threads, newest first.
    Threads,
}

#[derive(Subcommand)]
enum FoldersCommand {
    List,
    Add {
        path: PathBuf,
        #[arg(long)]
        label: Option<String>,
    },
    Remove {
        id: String,
    },
}

fn init_logging(default: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_env("G7_AGENTS_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}

/// Moves `host.log` to `host.log.1` once it passes the limit.
fn rotate_log() {
    let Some(dir) = client::state_dir() else { return };
    let log = dir.join("host.log");
    if std::fs::metadata(&log).is_ok_and(|m| m.len() > LOG_ROTATE_BYTES) {
        let _ = std::fs::rename(&log, dir.join("host.log.1"));
    }
}

fn runtime(threads: usize) -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread().worker_threads(threads).enable_all().build()
}

fn serve() -> ExitCode {
    rotate_log();
    init_logging("info");
    let config = match host::Config::from_env() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("g7-agents: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Ok(rt) = runtime(2) else { return ExitCode::FAILURE };
    match rt.block_on(host::serve(config, std::future::pending())) {
        Ok(served) => {
            if served == host::Served::AlreadyRunning {
                tracing::info!("another agent host is running");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!("the agent host stopped: {e}");
            ExitCode::FAILURE
        }
    }
}

fn mcp_approve() -> ExitCode {
    init_logging("warn");
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return ExitCode::FAILURE;
    };
    match rt.block_on(host::bridge::run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn describe(agent: &AgentInfo) -> String {
    if !agent.installed {
        return format!("{}: not installed", agent.name);
    }
    let version = agent.version.as_deref().unwrap_or("unknown version");
    let state = if agent.signed_in {
        match agent.auth {
            Some(AuthKind::ApiKey) => "using an API key".to_owned(),
            _ => "signed in".to_owned(),
        }
    } else {
        format!("signed out (run \"{}\")", agent.login_hint)
    };
    match &agent.problem {
        Some(problem) => format!("{} {version}: {state}; {problem}", agent.name),
        None => format!("{} {version}: {state}", agent.name),
    }
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().map(|dir| dir.join(path)).unwrap_or_else(|_| path.to_owned())
    }
}

fn client_command(command: Command) -> Result<(), ClientError> {
    let mut client = BlockingClient::connect_or_spawn(ClientKind::Cli)?;
    match command {
        Command::Agents { refresh } => {
            if refresh {
                client.set_timeout(std::time::Duration::from_secs(30));
            }
            for agent in client.agents_list(refresh)? {
                println!("{}", describe(&agent));
            }
        }
        Command::Folders { command: FoldersCommand::List } => {
            for folder in client.folders_list()? {
                let missing = if folder.exists { "" } else { " (missing)" };
                println!("{}  {}  {}{missing}", folder.id, folder.label, folder.path);
            }
        }
        Command::Folders { command: FoldersCommand::Add { path, label } } => {
            let folder = client.folders_add(absolute(&path).to_string_lossy().into_owned(), label)?;
            println!("{}  {}  {}", folder.id, folder.label, folder.path);
        }
        Command::Folders { command: FoldersCommand::Remove { id } } => client.folders_remove(id)?,
        Command::Threads => {
            for thread in client.threads_list(Some(100), None)? {
                let state =
                    serde_json::to_value(thread.state).ok().and_then(|v| v.as_str().map(str::to_owned));
                println!(
                    "{}  {}  {}  {}  {}",
                    thread.id,
                    thread.agent.display_name(),
                    state.unwrap_or_default(),
                    thread.folder_label,
                    thread.title
                );
            }
        }
        Command::Serve | Command::McpApprove => {}
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve => serve(),
        Command::McpApprove => mcp_approve(),
        command => match client_command(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("g7-agents: {e}");
                ExitCode::FAILURE
            }
        },
    }
}
