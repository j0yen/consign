//! consign — accurate fleet push-debt enumerator
#![deny(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use consign::drain::{drain_all, render_drain_table, DrainConfig, RealPushRunner};
use consign::policy::{render_policy_table, survey_with_policy, PolicyCfg};
use consign::verify::{render_verify_table, VerifyConfig};
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

    /// Verify convergence: re-survey fleet and check push-debt is gone.
    ///
    /// Verdicts:
    ///   converged   — no auto-ok push-debt remains (exit 0)
    ///   contradicted — a repo drain claimed pushed is still ahead/no-upstream (exit 1)
    ///   residual    — auto-ok debt remains but no drain receipt supplied (exit 0 + warning)
    ///
    /// manual-only/diverged/no-remote debt is always out of convergence scope.
    Verify {
        /// Root directories to scan (default: ~/wintermute)
        #[arg(long = "root", short = 'r')]
        roots: Vec<PathBuf>,

        /// Drain receipt JSON to cross-check against (output of: consign drain --format json)
        #[arg(long = "against")]
        against: Option<PathBuf>,

        /// Output format
        #[arg(long, short = 'f', default_value = "table")]
        format: Format,
    },

    /// Classify push-eligibility for each repo: auto-ok | private-hold | manual-only.
    ///
    /// Policy classes:
    ///   auto-ok       — safe to push automatically (ahead/no-upstream/no-remote on
    ///                   default branch, no hold marker, no detected secret)
    ///   private-hold  — name matches *-private or autobuilder*, .consign-hold file
    ///                   present, or a tracked secret-shaped file (.env, *.pem, id_*,
    ///                   *credential*) detected. Never auto-pushed.
    ///   manual-only   — diverged, non-default branch (worktree/feature), or detached
    ///                   HEAD. Needs human review.
    ///
    /// Config override: ~/.config/consign/policy.toml (or $CONSIGN_POLICY_CONFIG)
    /// can add extra_hold_globs and extra_hold_paths. Missing config is not an error.
    Policy {
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
        Cmd::Verify {
            mut roots,
            against,
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

            // Load drain receipt if --against was supplied.
            let drain_receipt = if let Some(ref path) = against {
                match std::fs::read_to_string(path) {
                    Ok(s) => match serde_json::from_str::<Vec<consign::drain::DrainReceipt>>(&s) {
                        Ok(r) => Some(r),
                        Err(e) => {
                            eprintln!("consign verify: failed to parse drain receipt {}: {}", path.display(), e);
                            std::process::exit(1);
                        }
                    },
                    Err(e) => {
                        eprintln!("consign verify: cannot read drain receipt {}: {}", path.display(), e);
                        std::process::exit(1);
                    }
                }
            } else {
                None
            };

            let config = VerifyConfig { roots, drain_receipt };
            match consign::verify::verify(&config) {
                Ok(receipt) => {
                    let exit_code = receipt.overall.exit_code();

                    match format {
                        Format::Json => {
                            println!("{}", serde_json::to_string_pretty(&receipt).unwrap());
                        }
                        Format::Table => {
                            print!("{}", render_verify_table(&receipt));
                        }
                    }

                    if exit_code != 0 {
                        std::process::exit(exit_code);
                    }
                }
                Err(e) => {
                    eprintln!("consign verify: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Cmd::Policy { mut roots, format } => {
            if roots.is_empty() {
                if let Some(home) = std::env::var_os("HOME") {
                    roots.push(PathBuf::from(home).join("wintermute"));
                } else {
                    eprintln!("consign: HOME not set; pass --root explicitly");
                    std::process::exit(1);
                }
            }

            let cfg = PolicyCfg::load();
            match survey_with_policy(&roots, &cfg) {
                Ok(entries) => match format {
                    Format::Json => {
                        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
                    }
                    Format::Table => {
                        print!("{}", render_policy_table(&entries));
                    }
                },
                Err(e) => {
                    eprintln!("consign policy: {}", e);
                    std::process::exit(1);
                }
            }
        }

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
