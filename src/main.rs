// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use rust_decimal::prelude::ToPrimitive;
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
    /// Show dividend history for a symbol
    Dividends {
        /// Ticker symbol (e.g. BSV, AAPL, VBTLX)
        symbol: String,

        /// Convert amounts to a currency (e.g. EUR, GBP)
        #[arg(short, long)]
        currency: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Time range (1mo, 6mo, 1y, 5y, max)
        #[arg(short, long, default_value = "1y")]
        range: String,
    },
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
    /// Show effective annual yield (price + dividends)
    Yield {
        /// Ticker symbol (e.g. BSV, AAPL, VBTLX)
        symbol: String,

        /// Convert to a currency (e.g. EUR, GBP, JPY)
        #[arg(short, long)]
        currency: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Time range (1y, 2y, 5y, max)
        #[arg(short, long, default_value = "1y")]
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
    cumulative_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    change_local: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cumulative_local: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exchange_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    currency: Option<String>,
}

#[derive(Serialize)]
struct DividendRecord {
    date: String,
    amount_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    amount_local: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exchange_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    currency: Option<String>,
}

#[derive(Serialize)]
struct YieldReport {
    symbol: String,
    range: String,
    days: i64,
    start_date: String,
    end_date: String,
    start_price_usd: f64,
    end_price_usd: f64,
    price_change_usd: f64,
    total_dividends_usd: f64,
    total_return_usd: f64,
    total_return_pct: f64,
    annualized_yield_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_price_local: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_price_local: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_change_local: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_dividends_local: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_return_local: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_return_local_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    annualized_yield_local_pct: Option<f64>,
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

/// Look up the exchange rate for a date, falling back to the
/// most recent known rate.
fn lookup_rate(
    fx_rates: &HashMap<String, f64>,
    date: &str,
    last_rate: &mut f64,
    currency_is_usd: bool,
) -> f64 {
    if currency_is_usd {
        1.0
    } else if let Some(&r) = fx_rates.get(date) {
        *last_rate = r;
        r
    } else {
        *last_rate
    }
}

fn format_change(val: f64) -> String {
    if val >= 0.0 {
        format!("+{val:.2}")
    } else {
        format!("{val:.2}")
    }
}

/// Annualize a total return percentage over a number of days.
/// Uses compound annual growth rate: (1 + r)^(365/days) - 1
fn annualize(total_return_pct: f64, days: i64) -> f64 {
    if days <= 0 {
        return 0.0;
    }
    let r = total_return_pct / 100.0;
    ((1.0 + r).powf(365.0 / days as f64) - 1.0) * 100.0
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Dividends {
            symbol,
            currency,
            json,
            range,
        } => {
            let provider = yahoo::YahooConnector::new()?;
            let response = provider.get_quote_range(&symbol, "1d", &range).await?;
            let dividends = response.dividends()?;

            let has_currency = currency.is_some();
            let currency_label = currency.as_ref().map(|c| c.to_uppercase());
            let currency_is_usd = currency_label.as_deref() == Some("USD");

            let fx_rates = if has_currency && !currency_is_usd {
                get_exchange_rates(
                    &provider,
                    "USD",
                    currency_label.as_deref().unwrap(),
                    "1d",
                    &range,
                )
                .await?
            } else {
                HashMap::new()
            };

            let mut last_rate = 1.0_f64;
            let mut total_usd = 0.0_f64;
            let mut total_local = 0.0_f64;
            let records: Vec<DividendRecord> = dividends
                .iter()
                .map(|d| {
                    let dt: DateTime<Utc> = DateTime::from_timestamp(d.date, 0).unwrap_or_default();
                    let date = dt.format("%Y-%m-%d").to_string();
                    let amount_usd = d.amount.to_f64().unwrap_or(0.0);
                    total_usd += amount_usd;

                    let (amount_local, exchange_rate) = if has_currency {
                        let rate = lookup_rate(&fx_rates, &date, &mut last_rate, currency_is_usd);
                        let local = amount_usd * rate;
                        total_local += local;
                        (Some(local), Some(rate))
                    } else {
                        (None, None)
                    };

                    DividendRecord {
                        date,
                        amount_usd,
                        amount_local,
                        exchange_rate,
                        currency: currency_label.clone(),
                    }
                })
                .collect();

            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else if has_currency {
                let cur = currency_label.as_deref().unwrap();
                println!(
                    "{:<12} {:>12} {:>12} {:>11}",
                    "Date",
                    "Amt(USD)",
                    format!("Amt({cur})"),
                    "Rate"
                );
                for r in &records {
                    println!(
                        "{:<12} {:>12.4} {:>12.4} {:>11.4}",
                        r.date,
                        r.amount_usd,
                        r.amount_local.unwrap_or(0.0),
                        r.exchange_rate.unwrap_or(1.0)
                    );
                }
                println!("{:<12} {:>12.4} {:>12.4}", "Total", total_usd, total_local);
            } else {
                println!("{:<12} {:>12}", "Date", "Amt(USD)");
                for r in &records {
                    println!("{:<12} {:>12.4}", r.date, r.amount_usd);
                }
                println!("{:<12} {:>12.4}", "Total", total_usd);
            }
        }
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
            let currency_is_usd = currency_label.as_deref() == Some("USD");

            let fx_rates = if has_currency && !currency_is_usd {
                get_exchange_rates(
                    &provider,
                    "USD",
                    currency_label.as_deref().unwrap(),
                    &interval,
                    &range,
                )
                .await?
            } else {
                HashMap::new()
            };

            let mut prev_close_usd: Option<f64> = None;
            let mut cumul_usd = 0.0_f64;
            let mut cumul_local = 0.0_f64;
            let mut last_rate = 1.0_f64;
            let records: Vec<PriceRecord> = quotes
                .iter()
                .map(|q| {
                    let dt: DateTime<Utc> =
                        DateTime::from_timestamp(q.timestamp as i64, 0).unwrap_or_default();
                    let date = dt.format("%Y-%m-%d").to_string();

                    let rate = if has_currency {
                        lookup_rate(&fx_rates, &date, &mut last_rate, currency_is_usd)
                    } else {
                        1.0
                    };

                    let change_usd = prev_close_usd.map(|prev| q.close - prev);
                    if let Some(c) = change_usd {
                        cumul_usd += c;
                    }
                    let change_local = if has_currency {
                        change_usd.map(|c| c * rate)
                    } else {
                        None
                    };
                    if let Some(c) = change_local {
                        cumul_local += c;
                    }
                    prev_close_usd = Some(q.close);

                    PriceRecord {
                        date,
                        open: q.open * rate,
                        high: q.high * rate,
                        low: q.low * rate,
                        close: q.close * rate,
                        volume: q.volume,
                        change_usd,
                        cumulative_usd: if prev_close_usd.is_some() {
                            Some(cumul_usd)
                        } else {
                            None
                        },
                        change_local,
                        cumulative_local: if has_currency && change_local.is_some() {
                            Some(cumul_local)
                        } else {
                            None
                        },
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
                    "{:<12} {:>10} {:>10} {:>10} {:>10} {:>12} \
                     {:>10} {:>10} {:>8} {:>8} {:>11}",
                    "Date",
                    format!("Open({cur})"),
                    format!("High({cur})"),
                    format!("Low({cur})"),
                    format!("Close({cur})"),
                    "Volume",
                    "Chg(USD)",
                    "Cum(USD)",
                    format!("Chg({cur})"),
                    format!("Cum({cur})"),
                    "Rate"
                );
                for r in &records {
                    let chg_usd = r.change_usd.map(format_change).unwrap_or_default();
                    let cum_usd = r.cumulative_usd.map(format_change).unwrap_or_default();
                    let chg_local = r.change_local.map(format_change).unwrap_or_default();
                    let cum_local = r.cumulative_local.map(format_change).unwrap_or_default();
                    let rate = r.exchange_rate.unwrap_or(1.0);
                    println!(
                        "{:<12} {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>12} \
                         {:>10} {:>10} {:>8} {:>8} {:>11.4}",
                        r.date,
                        r.open,
                        r.high,
                        r.low,
                        r.close,
                        r.volume,
                        chg_usd,
                        cum_usd,
                        chg_local,
                        cum_local,
                        rate
                    );
                }
            } else {
                println!(
                    "{:<12} {:>10} {:>10} {:>10} {:>10} {:>12} {:>10} {:>10}",
                    "Date", "Open", "High", "Low", "Close", "Volume", "Chg(USD)", "Cum(USD)"
                );
                for r in &records {
                    let chg = r.change_usd.map(format_change).unwrap_or_default();
                    let cum = r.cumulative_usd.map(format_change).unwrap_or_default();
                    println!(
                        "{:<12} {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>12} {:>10} {:>10}",
                        r.date, r.open, r.high, r.low, r.close, r.volume, chg, cum
                    );
                }
            }
        }
        Command::Yield {
            symbol,
            currency,
            json,
            range,
        } => {
            let provider = yahoo::YahooConnector::new()?;
            let response = provider.get_quote_range(&symbol, "1d", &range).await?;
            let quotes = response.quotes()?;
            let dividends = response.dividends()?;

            if quotes.len() < 2 {
                return Err("not enough price data".into());
            }

            let first = quotes.first().unwrap();
            let last = quotes.last().unwrap();

            let start_date =
                DateTime::from_timestamp(first.timestamp as i64, 0).unwrap_or_default();
            let end_date = DateTime::from_timestamp(last.timestamp as i64, 0).unwrap_or_default();
            let days = (end_date - start_date).num_days();

            let start_price_usd = first.close;
            let end_price_usd = last.close;
            let price_change_usd = end_price_usd - start_price_usd;
            let total_dividends_usd: f64 = dividends
                .iter()
                .map(|d| d.amount.to_f64().unwrap_or(0.0))
                .sum();
            let total_return_usd = price_change_usd + total_dividends_usd;
            let total_return_pct = (total_return_usd / start_price_usd) * 100.0;
            let annualized_yield_pct = annualize(total_return_pct, days);

            let has_currency = currency.is_some();
            let currency_label = currency.as_ref().map(|c| c.to_uppercase());
            let currency_is_usd = currency_label.as_deref() == Some("USD");

            let (
                start_price_local,
                end_price_local,
                price_change_local,
                total_dividends_local,
                total_return_local,
                total_return_local_pct,
                annualized_yield_local_pct,
            ) = if has_currency && !currency_is_usd {
                let fx_rates = get_exchange_rates(
                    &provider,
                    "USD",
                    currency_label.as_deref().unwrap(),
                    "1d",
                    &range,
                )
                .await?;

                let start_str = start_date.format("%Y-%m-%d").to_string();
                let end_str = end_date.format("%Y-%m-%d").to_string();
                let mut last_rate = 1.0_f64;
                let start_rate = lookup_rate(&fx_rates, &start_str, &mut last_rate, false);
                let end_rate = lookup_rate(&fx_rates, &end_str, &mut last_rate, false);

                let sp = start_price_usd * start_rate;
                let ep = end_price_usd * end_rate;
                let pc = ep - sp;

                let mut div_local = 0.0_f64;
                let mut lr = 1.0_f64;
                for d in &dividends {
                    let dt: DateTime<Utc> = DateTime::from_timestamp(d.date, 0).unwrap_or_default();
                    let date = dt.format("%Y-%m-%d").to_string();
                    let rate = lookup_rate(&fx_rates, &date, &mut lr, false);
                    div_local += d.amount.to_f64().unwrap_or(0.0) * rate;
                }

                let tr = pc + div_local;
                let tr_pct = (tr / sp) * 100.0;
                let ay_pct = annualize(tr_pct, days);

                (
                    Some(sp),
                    Some(ep),
                    Some(pc),
                    Some(div_local),
                    Some(tr),
                    Some(tr_pct),
                    Some(ay_pct),
                )
            } else if has_currency {
                // USD explicitly requested
                (
                    Some(start_price_usd),
                    Some(end_price_usd),
                    Some(price_change_usd),
                    Some(total_dividends_usd),
                    Some(total_return_usd),
                    Some(total_return_pct),
                    Some(annualized_yield_pct),
                )
            } else {
                (None, None, None, None, None, None, None)
            };

            let report = YieldReport {
                symbol: symbol.clone(),
                range: range.clone(),
                days,
                start_date: start_date.format("%Y-%m-%d").to_string(),
                end_date: end_date.format("%Y-%m-%d").to_string(),
                start_price_usd,
                end_price_usd,
                price_change_usd,
                total_dividends_usd,
                total_return_usd,
                total_return_pct,
                annualized_yield_pct,
                start_price_local,
                end_price_local,
                price_change_local,
                total_dividends_local,
                total_return_local,
                total_return_local_pct,
                annualized_yield_local_pct,
                currency: currency_label.clone(),
            };

            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("{} — effective yield over {}", symbol, range);
                println!(
                    "Period:       {} to {} ({} days)",
                    report.start_date, report.end_date, days
                );
                println!();
                println!("  USD:");
                println!("    Start price:      {:>10.2}", start_price_usd);
                println!("    End price:        {:>10.2}", end_price_usd);
                println!("    Price change:     {:>10.2}", price_change_usd);
                println!("    Dividends:        {:>10.2}", total_dividends_usd);
                println!(
                    "    Total return:     {:>10.2} ({:+.2}%)",
                    total_return_usd, total_return_pct
                );
                println!(
                    "    Annualized yield: {:>10}  ({:+.2}%)",
                    "", annualized_yield_pct
                );

                if let Some(cur) = currency_label.as_deref() {
                    if cur != "USD" {
                        println!();
                        println!("  {cur}:");
                        println!("    Start price:      {:>10.2}", start_price_local.unwrap());
                        println!("    End price:        {:>10.2}", end_price_local.unwrap());
                        println!(
                            "    Price change:     {:>10.2}",
                            price_change_local.unwrap()
                        );
                        println!(
                            "    Dividends:        {:>10.2}",
                            total_dividends_local.unwrap()
                        );
                        println!(
                            "    Total return:     {:>10.2} ({:+.2}%)",
                            total_return_local.unwrap(),
                            total_return_local_pct.unwrap()
                        );
                        println!(
                            "    Annualized yield: {:>10}  ({:+.2}%)",
                            "",
                            annualized_yield_local_pct.unwrap()
                        );
                    }
                }
            }
        }
    }

    Ok(())
}
