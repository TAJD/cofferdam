//! Cofferdam CLI entry point.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use cofferdam_checks::all_builtins;
use cofferdam_engine::{discover, DiscoveryOptions, Engine};
use cofferdam_formatters::{JsonFormatter, TextFormatter};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// Human-readable text grouped by category (default).
    Text,
    /// Machine-readable JSON. Stable schema, no ANSI, no decorative output.
    Json,
}

const BANNER: &str = "\
  ┌─────────────────────────────────────────────────┐
  │  cofferdam v{version}                              │
  │  isolate bad code below the waterline           │
  │  measure it · sort it · ship the verdict        │
  └─────────────────────────────────────────────────┘";

#[derive(Parser)]
#[command(
    name = "cofferdam",
    version,
    about = "TypeScript code-quality analyzer"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the project banner.
    Hello,
    /// Run all checks against files or directories. With no arguments,
    /// walks the current directory.
    Check {
        /// Files or directories to analyze. Defaults to `.`.
        paths: Vec<PathBuf>,
        /// Walk hidden files/directories (default: skip).
        #[arg(long)]
        hidden: bool,
        /// Disable `.gitignore` / `.cofferdamignore` filtering.
        #[arg(long)]
        no_ignore: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        /// Shorthand for `--format=json`. Token-economical output for AI agents.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output (only with `--format=json` / `--robot`).
        #[arg(long)]
        pretty: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Cmd::Hello => {
            println!("{}", BANNER.replace("{version}", env!("CARGO_PKG_VERSION")));
            ExitCode::SUCCESS
        }
        Cmd::Check {
            paths,
            hidden,
            no_ignore,
            format,
            robot,
            pretty,
        } => {
            // `--robot` is the user-facing flag; resolve to a single format.
            let format = if robot { OutputFormat::Json } else { format };
            let roots: Vec<PathBuf> = if paths.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                paths
            };
            let opts = DiscoveryOptions {
                respect_ignore: !no_ignore,
                include_hidden: hidden,
                ..DiscoveryOptions::default()
            };
            let files = match discover(&roots, &opts) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            };
            if files.is_empty() {
                if format == OutputFormat::Json {
                    // Empty-but-valid JSON document so agents can still parse.
                    println!(r#"{{"findings":[],"summary":{{"total":0,"by_category":{{}}}}}}"#);
                } else {
                    eprintln!("no TypeScript files found under: {:?}", roots);
                }
                return ExitCode::SUCCESS;
            }

            let engine = Engine::new(all_builtins());
            match engine.analyze(&files) {
                Ok(issues) => {
                    match format {
                        OutputFormat::Text => print!("{}", TextFormatter::render(&issues)),
                        OutputFormat::Json => {
                            let s = if pretty {
                                JsonFormatter::render_pretty(&issues)
                            } else {
                                JsonFormatter::render(&issues)
                            };
                            println!("{}", s);
                        }
                    }
                    if issues.is_empty() {
                        ExitCode::SUCCESS
                    } else {
                        // Phase 0: any finding fails. Severity gating arrives in phase 3.
                        ExitCode::from(1)
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::from(2)
                }
            }
        }
    }
}
