// SPDX-License-Identifier: MPL-2.0

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;
use yahoo_finance_api as yahoo;

#[derive(Parser)]
#[command(
    version,
    about,
    long_about = None,
    before_help = concat!(
        env!("CARGO_PKG_NAME"), " ",
        env!("CARGO_PKG_VERSION")
    )
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show historical prices for a symbol
    History {
        /// Ticker symbol (e.g. BSV, AAPL, VBTLX)
        symbol: String,

        /// Time range: 1d, 5d, 1mo, 3mo, 6mo, 1y, 2y,
        /// 5y, 10y, max
        #[arg(short, long, default_value = "6mo")]
        range: String,

        /// Interval: 1d, 1wk, 1mo
        #[arg(short, long, default_value = "1d")]
        interval: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Serialize)]
struct PriceRecord {
    date: String,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::History {
            symbol,
            range,
            interval,
            json,
        } => {
            let provider = yahoo::YahooConnector::new()?;
            let response = provider.get_quote_range(&symbol, &interval, &range).await?;
            let quotes = response.quotes()?;

            let records: Vec<PriceRecord> = quotes
                .iter()
                .map(|q| {
                    let dt: DateTime<Utc> =
                        DateTime::from_timestamp(q.timestamp as i64, 0).unwrap_or_default();
                    PriceRecord {
                        date: dt.format("%Y-%m-%d").to_string(),
                        open: q.open,
                        high: q.high,
                        low: q.low,
                        close: q.close,
                        volume: q.volume,
                    }
                })
                .collect();

            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else {
                println!(
                    "{:<12} {:>10} {:>10} {:>10} {:>10} {:>12}",
                    "Date", "Open", "High", "Low", "Close", "Volume"
                );
                for r in &records {
                    println!(
                        "{:<12} {:>10.2} {:>10.2} {:>10.2} \
                         {:>10.2} {:>12}",
                        r.date, r.open, r.high, r.low, r.close, r.volume
                    );
                }
            }
        }
    }

    Ok(())
}
