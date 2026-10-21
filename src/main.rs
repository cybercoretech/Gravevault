//! Gravevault: backups you've proven you can restore.
//!
//! Milestone 1: the append-only GitHub → GitLab mirror (formerly the
//! `cybercore-gitlab-backup` bash script), reporting to Warnflare.

mod config;
mod mirror;
mod report;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "gravevault",
    version,
    about = "Backups you've proven you can restore"
)]
struct Cli {
    /// Config file (default: ~/.config/gravevault/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Mirror every GitHub repo to the GitLab group (append-only).
    Mirror,
    /// Show the last run of each job.
    Status,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let path = cli.config.unwrap_or_else(config::default_path);
    let cfg = match config::Config::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("✗ {e}");
            return ExitCode::FAILURE;
        }
    };
    match cli.cmd {
        Cmd::Mirror => {
            let Some(m) = &cfg.git_mirror else {
                eprintln!("✗ no [git_mirror] section in {}", path.display());
                return ExitCode::FAILURE;
            };
            let state = config::state_dir();
            let _ = std::fs::create_dir_all(&state);
            let Some(_lock) = report::Lock::take(&state.join("git-mirror.lock")) else {
                println!("another mirror run is in progress");
                return ExitCode::SUCCESS;
            };
            let mut log = |l: &str| println!("{} {l}", &mirror::utc_now()[11..19]);
            let r = mirror::run(m, &mirror::stamp(), &mut log);
            println!(
                "{} backed up {}/{} repositories; created {}; rewritten {}; failed {}",
                &mirror::utc_now()[11..19],
                r.ok,
                r.total,
                r.created.len(),
                r.rewritten.len(),
                r.failed.len()
            );
            report::save(&state.join("git-mirror.json"), &r);
            if let Some(w) = &cfg.warnflare {
                report::notify(w, "git-mirror", &r);
            }
            if r.failed.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Cmd::Status => {
            match std::fs::read_to_string(config::state_dir().join("git-mirror.json")) {
                Ok(s) => println!("git-mirror  {s}"),
                Err(_) => println!("git-mirror  never run"),
            }
            ExitCode::SUCCESS
        }
    }
}
