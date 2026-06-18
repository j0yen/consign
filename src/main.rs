//! consign — accurate fleet push-debt enumerator
#![deny(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use consign::drain::{drain_all, render_drain_table, DrainConfig, RealPushRunner};
use consign::{render_table, survey};

#[derive(Debug, Clone, ValueEnum)]
enum Format {
    Json,
    Table,
}

#[derive(Debug, Parser)]
#[command(
    name = "consign",
    about = "Accurate fleet push-debt enumerator for git repos",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Walk git repos and classify their push-debt
    Survey {
        /// Root directories to scan (default: ~/wintermute)
        #[arg(long = "root", short = 'r')]
        roots: Vec<PathBuf>,

        /// Output format
        #[arg(long, short = 'f', default_value = "table")]
        format: Format,
    },

    /// Push eligible repos safely (dry-run by default; use --no-dry-run to push).
    ///
    /// Runs survey+policy internally. For each auto-ok repo classified as
    /// 'ahead' or 'no-upstream', the push plan is printed (dry-run) or executed
    /// (--no-dry-run). Diverged and manual-only repos are NEVER pushed and
    /// appear in a "needs human" section. This command NEVER uses --force or
    /// --force-with-lease.
    Drain {
        /// Root directories to scan (default: ~/wintermute)
        #[arg(long = "root", short = 'r')]
        roots: Vec<PathBuf>,

        /// Only drain repos with these names (repeatable)
        #[arg(long = "only")]
        only: Vec<String>,

        /// Perform pushes (default is dry-run — print plan only)
        #[arg(long = "no-dry-run", action = clap::ArgAction::SetTrue)]
        no_dry_run: bool,

        /// Output format
        #[arg(long, short = 'f', default_value = "table")]
        format: Format,
    },
}

fn main() {
    // SIGPIPE reset MUST be the first line of main() (see memory self_sigpipe_panic_toolkit)
    sigpipe::reset();

    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Survey { mut roots, format } => {
            if roots.is_empty() {
                if let Some(home) = std::env::var_os("HOME") {
                    roots.push(PathBuf::from(home).join("wintermute"));
                } else {
                    eprintln!("consign: HOME not set; pass --root explicitly");
                    std::process::exit(1);
                }
            }

            match survey(&roots) {
                Ok(debts) => match format {
                    Format::Json => {
                        println!("{}", serde_json::to_string_pretty(&debts).unwrap());
                    }
                    Format::Table => {
                        print!("{}", render_table(&debts));
                    }
                },
                Err(e) => {
                    eprintln!("consign: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Cmd::Drain {
            mut roots,
            only,
            no_dry_run,
            format,
        } => {
            if roots.is_empty() {
                if let Some(home) = std::env::var_os("HOME") {
                    roots.push(PathBuf::from(home).join("wintermute"));
                } else {
                    eprintln!("consign: HOME not set; pass --root explicitly");
                    std::process::exit(1);
                }
            }

            let dry_run = !no_dry_run;
            if dry_run {
                eprintln!("consign drain: dry-run mode (use --no-dry-run to push)");
            }

            match survey(&roots) {
                Ok(debts) => {
                    let runner = RealPushRunner;
                    let config = DrainConfig {
                        dry_run,
                        only: &only,
                        runner: &runner,
                    };

                    let (receipts, needs_human) = drain_all(&debts, &config);
                    let has_errors = receipts
                        .iter()
                        .any(|r| r.result == consign::drain::ActionResult::Error);

                    match format {
                        Format::Json => {
                            // Combine receipts and needs_human into one array
                            let mut all = receipts.clone();
                            all.extend(needs_human.iter().cloned());
                            println!("{}", serde_json::to_string_pretty(&all).unwrap());
                        }
                        Format::Table => {
                            print!("{}", render_drain_table(&receipts, &needs_human));
                        }
                    }

                    if has_errors {
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("consign: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}
