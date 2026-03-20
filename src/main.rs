// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;

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
    change_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    change_local: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exchange_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    currency: Option<String>,
}

/// Fetch historical exchange rates and return a map from
/// date string (YYYY-MM-DD) to closing rate.
async fn get_exchange_rates(
    provider: &yahoo::YahooConnector,
    from: &str,
    to: &str,
    interval: &str,
    range: &str,
) -> Result<HashMap<String, f64>, Box<dyn std::error::Error>> {
    let pair = format!("{from}{to}=X");
    let response = provider.get_quote_range(&pair, interval, range).await?;
    let quotes = response.quotes()?;
    let mut rates = HashMap::new();
    for q in &quotes {
        let dt: DateTime<Utc> = DateTime::from_timestamp(q.timestamp as i64, 0).unwrap_or_default();
        let date = dt.format("%Y-%m-%d").to_string();
        rates.insert(date, q.close);
    }
    Ok(rates)
}

fn format_change(val: f64) -> String {
    if val >= 0.0 {
        format!("+{val:.2}")
    } else {
        format!("{val:.2}")
    }
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

            let has_currency = currency.is_some();
            let currency_label = currency.as_ref().map(|c| c.to_uppercase());

            let fx_rates = if let Some(cur) = &currency_label {
                if cur == "USD" {
                    HashMap::new()
                } else {
                    get_exchange_rates(&provider, "USD", cur, &interval, &range).await?
                }
            } else {
                HashMap::new()
            };

            let mut prev_close_usd: Option<f64> = None;
            let mut last_rate = 1.0_f64;
            let records: Vec<PriceRecord> = quotes
                .iter()
                .map(|q| {
                    let dt: DateTime<Utc> =
                        DateTime::from_timestamp(q.timestamp as i64, 0).unwrap_or_default();
                    let date = dt.format("%Y-%m-%d").to_string();

                    let rate = if has_currency {
                        if let Some(cur) = &currency_label {
                            if cur == "USD" {
                                1.0
                            } else if let Some(&r) = fx_rates.get(&date) {
                                last_rate = r;
                                r
                            } else {
                                // Use last known rate for
                                // missing days
                                last_rate
                            }
                        } else {
                            1.0
                        }
                    } else {
                        1.0
                    };

                    let change_usd = prev_close_usd.map(|prev| q.close - prev);
                    let change_local = if has_currency {
                        change_usd.map(|c| c * rate)
                    } else {
                        None
                    };
                    prev_close_usd = Some(q.close);

                    PriceRecord {
                        date,
                        open: q.open * rate,
                        high: q.high * rate,
                        low: q.low * rate,
                        close: q.close * rate,
                        volume: q.volume,
                        change_usd,
                        change_local,
                        exchange_rate: if has_currency { Some(rate) } else { None },
                        currency: currency_label.clone(),
                    }
                })
                .collect();

            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else if has_currency {
                let cur = currency_label.as_deref().unwrap();
                println!(
                    "{:<12} {:>10} {:>10} {:>10} {:>10} \
                     {:>12} {:>10} {:>8} {:>11}",
                    "Date",
                    format!("Open({cur})"),
                    format!("High({cur})"),
                    format!("Low({cur})"),
                    format!("Close({cur})"),
                    "Volume",
                    "Chg(USD)",
                    format!("Chg({cur})"),
                    "Rate"
                );
                for r in &records {
                    let chg_usd = r.change_usd.map(format_change).unwrap_or_default();
                    let chg_local = r.change_local.map(format_change).unwrap_or_default();
                    let rate = r.exchange_rate.unwrap_or(1.0);
                    println!(
                        "{:<12} {:>10.2} {:>10.2} {:>10.2} \
                         {:>10.2} {:>12} {:>10} {:>8} \
                         {:>11.4}",
                        r.date, r.open, r.high, r.low, r.close, r.volume, chg_usd, chg_local, rate
                    );
                }
            } else {
                println!(
                    "{:<12} {:>10} {:>10} {:>10} {:>10} \
                     {:>12} {:>10}",
                    "Date", "Open", "High", "Low", "Close", "Volume", "Chg(USD)"
                );
                for r in &records {
                    let chg = r.change_usd.map(format_change).unwrap_or_default();
                    println!(
                        "{:<12} {:>10.2} {:>10.2} {:>10.2} \
                         {:>10.2} {:>12} {:>10}",
                        r.date, r.open, r.high, r.low, r.close, r.volume, chg
                    );
                }
            }
        }
    }

    Ok(())
}
