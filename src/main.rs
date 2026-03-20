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

        /// Convert prices to a currency (e.g. EUR, GBP,
        /// JPY)
        #[arg(short, long)]
        currency: Option<String>,

        /// Interval: 1d, 1wk, 1mo
        #[arg(short, long, default_value = "1d")]
        interval: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Time range (1mo, 6mo, 1y, 5y, max)
        #[arg(short, long, default_value = "6mo")]
        range: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    currency: Option<String>,
}

async fn get_exchange_rate(
    provider: &yahoo::YahooConnector,
    from: &str,
    to: &str,
) -> Result<f64, Box<dyn std::error::Error>> {
    let pair = format!("{from}{to}=X");
    let response = provider.get_latest_quotes(&pair, "1d").await?;
    let quote = response.last_quote()?;
    Ok(quote.close)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::History {
            symbol,
            currency,
            interval,
            json,
            range,
        } => {
            let provider = yahoo::YahooConnector::new()?;
            let response = provider.get_quote_range(&symbol, &interval, &range).await?;
            let quotes = response.quotes()?;

            let rate = match &currency {
                Some(cur) => {
                    let cur = cur.to_uppercase();
                    if cur == "USD" {
                        1.0
                    } else {
                        get_exchange_rate(&provider, "USD", &cur).await?
                    }
                }
                None => 1.0,
            };

            let currency_label = currency.as_ref().map(|c| c.to_uppercase());

            let records: Vec<PriceRecord> = quotes
                .iter()
                .map(|q| {
                    let dt: DateTime<Utc> =
                        DateTime::from_timestamp(q.timestamp as i64, 0).unwrap_or_default();
                    PriceRecord {
                        date: dt.format("%Y-%m-%d").to_string(),
                        open: q.open * rate,
                        high: q.high * rate,
                        low: q.low * rate,
                        close: q.close * rate,
                        volume: q.volume,
                        currency: currency_label.clone(),
                    }
                })
                .collect();

            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else {
                let cur_suffix = currency_label
                    .as_deref()
                    .map(|c| format!(" ({c})"))
                    .unwrap_or_default();
                println!(
                    "{:<12} {:>10} {:>10} {:>10} {:>10} {:>12}",
                    "Date",
                    format!("Open{cur_suffix}"),
                    format!("High{cur_suffix}"),
                    format!("Low{cur_suffix}"),
                    format!("Close{cur_suffix}"),
                    "Volume"
                );
                for r in &records {
                    println!(
                        "{:<12} {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>12}",
                        r.date, r.open, r.high, r.low, r.close, r.volume
                    );
                }
            }
        }
    }

    Ok(())
}
