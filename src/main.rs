//! consign — accurate fleet push-debt enumerator
#![deny(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

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
    }
}
