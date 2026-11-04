use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use spoons_api_client::Client;

mod dump;
mod mcp;
mod orders;
mod prices;
mod tiers;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Save every pub's details, ale list, wait times and menus as JSON, then flatten the drinks into prices.jsonl.
    /// Resumable: anything already saved is skipped.
    Dump {
        #[arg(long, default_value = "data")]
        out: PathBuf,
        /// Which menus to save.
        #[arg(long, value_enum, default_value_t = Menus::Drinks)]
        menus: Menus,
        /// Least time between requests.
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
        /// Only these pubs, by venue ref.
        #[arg(long, value_delimiter = ',')]
        only: Vec<u64>,
    },
    /// Rebuild prices.jsonl from menus already dumped.
    Prices {
        #[arg(long, default_value = "data")]
        data: PathBuf,
    },
    /// Serve the MCP tools for finding a pub, reading its menu and ordering to a table, over stdio.
    Mcp {
        /// Serve over streamable HTTP at `/mcp` on this address instead, such as `0.0.0.0:8080`. Each request's
        /// `X-Spoons-Email` header says where its receipt goes. Only behind a proxy that authenticates callers.
        #[arg(long)]
        http: Option<String>,
        /// Where quotes and payments are kept; `$XDG_STATE_HOME/spoons` when not given.
        #[arg(long)]
        state: Option<PathBuf>,
    },
    /// Rank every pub by what a round of staples costs there, into tiers.md and tiers.json.
    Tiers {
        #[arg(long, default_value = "data")]
        data: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Menus {
    /// The drinks and real ale menus.
    Drinks,
    /// Every menu, food included. Around ten times the size.
    All,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::Dump {
            out,
            menus,
            interval_ms,
            only,
        } => {
            let client = Client::discover()
                .await?
                .with_interval(Duration::from_millis(interval_ms));
            dump::run(&client, &out, menus, &only).await?;
            prices::build(&out)
        }
        Command::Prices { data } => prices::build(&data),
        Command::Tiers { data } => tiers::build(&data),
        Command::Mcp { http, state } => mcp::serve(http.as_deref(), state).await,
    }
}
