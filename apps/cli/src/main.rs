//! `tickrail`: the command-line launcher.
//!
//! ```text
//! tickrail run      [--config FILE] [--set key=value ...]   start a session
//! tickrail replay   JOURNAL [--config FILE]                 deterministic backtest
//! tickrail export   JOURNAL OUT.csv [--format events|feed]  journal to CSV
//! tickrail list                                             compiled-in components
//! tickrail check    [--config FILE]                         validate a config
//! ```

mod config;
mod export;
mod replay;
mod run;
mod summary;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "tickrail", version, about = "Low-latency trading engine with pluggable venues and strategies")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start a session. With no --config, runs the built-in exchange simulator.
    Run {
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Override a config value, e.g. --set feed.symbol=ETHUSDT (repeatable).
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
    },
    /// Replay a journal through the strategy and risk settings of a config.
    Replay {
        journal: PathBuf,
        #[arg(short, long)]
        config: Option<PathBuf>,
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
    },
    /// Convert a journal to CSV.
    Export {
        journal: PathBuf,
        out: PathBuf,
        /// `events`: every record (orders and fills too). `feed`: market data only,
        /// in the format the `csv` adapter reads.
        #[arg(long, value_enum, default_value = "events")]
        format: ExportFormat,
    },
    /// List the adapters, venues and strategies in this build.
    List,
    /// Validate a config file without starting anything.
    Check {
        #[arg(short, long)]
        config: Option<PathBuf>,
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
    },
}

#[derive(Copy, Clone, ValueEnum)]
enum ExportFormat {
    Events,
    Feed,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Run { config, set } => {
            let (cfg, origin) = config::load(config.as_deref(), &set)?;
            eprintln!("[tickrail] config: {origin}");
            run::run(cfg)
        }
        Cmd::Replay { journal, config, set } => {
            let (cfg, _) = config::load(config.as_deref(), &set)?;
            replay::run(&journal, cfg)
        }
        Cmd::Export { journal, out, format } => export::run(&journal, &out, matches!(format, ExportFormat::Feed)),
        Cmd::List => {
            println!("feed adapters : simulator, {}", tickrail_adapters::available().join(", "));
            println!("venues        : paper, simulator");
            println!("strategies    :");
            for (name, what) in tickrail_strategies::registry::BUILTIN {
                println!("  {name:<14} {what}");
            }
            println!("wait modes    : spin, yield, backoff, sleep:<n>us");
            Ok(())
        }
        Cmd::Check { config, set } => {
            let (cfg, origin) = config::load(config.as_deref(), &set)?;
            run::validate(&cfg)?;
            println!("{origin}: ok");
            Ok(())
        }
    }
}
