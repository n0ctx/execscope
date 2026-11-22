mod brief;
mod engine;
mod harness;
mod lookup;
mod managed;
mod mcp;
mod model;
mod network;
mod platform;
mod probe;
mod project;
mod safe;
mod tools;
mod versions;

use clap::{Parser, Subcommand};
use model::{Detail, Request, Scope, Tool};
use std::{
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};

#[derive(Parser)]
#[command(
    version,
    about = "Inspect the local process environment; no command is equivalent to inspect."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Report local facts. Only --scope network performs outbound diagnostics; --scope harness focuses on agent harnesses.
    Inspect {
        #[arg(long, value_enum, default_value_t)]
        scope: Scope,
        #[arg(long, value_enum, default_value_t)]
        detail: Detail,
        /// Inspect this local directory instead of the process working directory.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Recollect requested facts (the standalone CLI has no persistent cache).
        #[arg(long)]
        refresh: bool,
        /// Check comma-separated lowercase prerequisites (for example: node,pnpm,git).
        #[arg(long, value_name = "TOOL[,TOOL...]")]
        requires: Option<String>,
        /// Approximate token budget for the brief report, 100 to 3000 (default 600).
        #[arg(long, value_name = "N")]
        brief_tokens: Option<u16>,
    },
    /// Serve the inspect_environment tool over MCP stdio.
    Mcp,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            return if error.print().is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
        Err(error) => {
            use clap::error::ErrorKind;
            let message = match error.kind() {
                ErrorKind::InvalidValue | ErrorKind::ValueValidation => "unsupported option value",
                ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand => {
                    "unknown command or option"
                }
                ErrorKind::InvalidUtf8 => "a command or option is not valid UTF-8",
                ErrorKind::TooFewValues | ErrorKind::MissingRequiredArgument => {
                    "missing option value"
                }
                _ => "invalid command line",
            };
            eprintln!("error: {message}; see execscope --help or execscope inspect --help");
            return ExitCode::from(2);
        }
    };
    let mcp = matches!(cli.command, Some(Command::Mcp));
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .max_blocking_threads(12)
        .enable_all()
        .build()
    else {
        eprintln!("error: could not start the inspection runtime");
        return ExitCode::FAILURE;
    };
    let result = runtime.block_on(async {
        match cli.command {
            Some(Command::Mcp) => mcp::serve().await,
            command => {
                let request = match command {
                    None => Request::default(),
                    Some(Command::Inspect {
                        scope,
                        detail,
                        cwd,
                        refresh,
                        requires,
                        brief_tokens,
                    }) => Request {
                        scope,
                        detail,
                        cwd,
                        refresh,
                        requires: requires.as_deref().map(parse_requirements).transpose()?,
                        brief_tokens,
                    },
                    Some(Command::Mcp) => unreachable!(),
                };
                let output = engine::Engine::new(false).inspect(&request).await?;
                writeln!(io::stdout().lock(), "{output}")
                    .map_err(|_| "could not write the environment report")
            }
        }
    });
    // A stuck filesystem/OS resolver cannot be preempted; do not wait indefinitely on process shutdown.
    runtime.shutdown_timeout(Duration::from_millis(100));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            if mcp {
                ExitCode::FAILURE
            } else {
                ExitCode::from(2)
            }
        }
    }
}

fn parse_requirements(value: &str) -> Result<Vec<Tool>, &'static str> {
    if value.len() > 256 {
        return Err("requires must be a short comma-separated list");
    }
    if value.is_empty() {
        return Ok(vec![]);
    }
    value.split(',').map(str::parse).collect()
}
