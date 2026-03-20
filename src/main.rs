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

        /// Convert to currency (repeatable)
        #[arg(short, long)]
        currency: Vec<String>,

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

        /// Convert to currency (repeatable)
        #[arg(short, long)]
        currency: Vec<String>,

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

        /// Convert to currency (repeatable)
        #[arg(short, long)]
        currency: Vec<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Time range (1y, 2y, 5y, max)
        #[arg(short, long, default_value = "1y")]
        range: String,
    },
}

// -- Data structures for JSON output --

#[derive(Serialize)]
struct CurrencyValues {
    currency: String,
    close: f64,
    change: Option<f64>,
    cumulative: Option<f64>,
    exchange_rate: f64,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    currencies: Vec<CurrencyValues>,
}

#[derive(Serialize)]
struct DividendCurrency {
    currency: String,
    amount: f64,
    exchange_rate: f64,
}

#[derive(Serialize)]
struct DividendRecord {
    date: String,
    amount_usd: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    currencies: Vec<DividendCurrency>,
}

#[derive(Serialize)]
struct DividendSummary {
    dividends: Vec<DividendRecord>,
    total_usd: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    totals: Vec<DividendCurrencyTotal>,
}

#[derive(Serialize)]
struct DividendCurrencyTotal {
    currency: String,
    total: f64,
}

#[derive(Serialize)]
struct CurrencyYield {
    currency: String,
    start_price: f64,
    end_price: f64,
    price_change: f64,
    total_dividends: f64,
    total_return: f64,
    total_return_pct: f64,
    annualized_yield_pct: f64,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    currencies: Vec<CurrencyYield>,
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

/// Fetch exchange rates for multiple currencies, skipping USD.
async fn get_all_exchange_rates(
    provider: &yahoo::YahooConnector,
    currencies: &[String],
    interval: &str,
    range: &str,
) -> Result<HashMap<String, HashMap<String, f64>>, Box<dyn std::error::Error>> {
    let mut all_rates = HashMap::new();
    for cur in currencies {
        if cur == "USD" {
            continue;
        }
        let rates = get_exchange_rates(provider, "USD", cur, interval, range).await?;
        all_rates.insert(cur.clone(), rates);
    }
    Ok(all_rates)
}

/// Look up the exchange rate for a date, falling back to the
/// most recent known rate.
fn lookup_rate(fx_rates: &HashMap<String, f64>, date: &str, last_rate: &mut f64) -> f64 {
    if let Some(&r) = fx_rates.get(date) {
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

/// Normalize currency list: uppercase, deduplicate.
fn normalize_currencies(currencies: &[String]) -> Vec<String> {
    let mut seen = Vec::new();
    for c in currencies {
        let upper = c.to_uppercase();
        if !seen.contains(&upper) {
            seen.push(upper);
        }
    }
    seen
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
            let currencies = normalize_currencies(&currency);
            let provider = yahoo::YahooConnector::new()?;
            let response = provider.get_quote_range(&symbol, "1d", &range).await?;
            let dividends = response.dividends()?;

            let all_rates = get_all_exchange_rates(&provider, &currencies, "1d", &range).await?;

            // Track last known rate per currency
            let mut last_rates: HashMap<String, f64> =
                currencies.iter().map(|c| (c.clone(), 1.0)).collect();
            let mut totals: HashMap<String, f64> =
                currencies.iter().map(|c| (c.clone(), 0.0)).collect();
            let mut total_usd = 0.0_f64;

            let records: Vec<DividendRecord> = dividends
                .iter()
                .map(|d| {
                    let dt: DateTime<Utc> = DateTime::from_timestamp(d.date, 0).unwrap_or_default();
                    let date = dt.format("%Y-%m-%d").to_string();
                    let amount_usd = d.amount.to_f64().unwrap_or(0.0);
                    total_usd += amount_usd;

                    let cur_values: Vec<DividendCurrency> = currencies
                        .iter()
                        .map(|cur| {
                            let rate = if cur == "USD" {
                                1.0
                            } else {
                                let fx = all_rates.get(cur).unwrap();
                                lookup_rate(fx, &date, last_rates.get_mut(cur).unwrap())
                            };
                            let amount = amount_usd * rate;
                            *totals.get_mut(cur).unwrap() += amount;
                            DividendCurrency {
                                currency: cur.clone(),
                                amount,
                                exchange_rate: rate,
                            }
                        })
                        .collect();

                    DividendRecord {
                        date,
                        amount_usd,
                        currencies: cur_values,
                    }
                })
                .collect();

            if json {
                let summary = DividendSummary {
                    dividends: records,
                    total_usd,
                    totals: currencies
                        .iter()
                        .map(|c| DividendCurrencyTotal {
                            currency: c.clone(),
                            total: totals[c],
                        })
                        .collect(),
                };
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else if currencies.is_empty() {
                println!("{:<12} {:>12}", "Date", "Amt(USD)");
                for r in &records {
                    println!("{:<12} {:>12.4}", r.date, r.amount_usd);
                }
                println!("{:<12} {:>12.4}", "Total", total_usd);
            } else {
                // Header
                print!("{:<12} {:>12}", "Date", "Amt(USD)");
                for cur in &currencies {
                    print!(" {:>12} {:>8}", format!("Amt({cur})"), format!("R({cur})"));
                }
                println!();
                // Rows
                for r in &records {
                    print!("{:<12} {:>12.4}", r.date, r.amount_usd);
                    for cv in &r.currencies {
                        print!(" {:>12.4} {:>8.4}", cv.amount, cv.exchange_rate);
                    }
                    println!();
                }
                // Totals
                print!("{:<12} {:>12.4}", "Total", total_usd);
                for cur in &currencies {
                    print!(" {:>12.4} {:>8}", totals[cur], "");
                }
                println!();
            }
        }
        Command::History {
            symbol,
            currency,
            interval,
            json,
            range,
        } => {
            let currencies = normalize_currencies(&currency);
            let provider = yahoo::YahooConnector::new()?;
            let response = provider.get_quote_range(&symbol, &interval, &range).await?;
            let quotes = response.quotes()?;

            let all_rates =
                get_all_exchange_rates(&provider, &currencies, &interval, &range).await?;

            let mut prev_close_usd: Option<f64> = None;
            let mut cumul_usd = 0.0_f64;

            // Per-currency tracking
            let mut last_rates: HashMap<String, f64> =
                currencies.iter().map(|c| (c.clone(), 1.0)).collect();
            let mut cumul_local: HashMap<String, f64> =
                currencies.iter().map(|c| (c.clone(), 0.0)).collect();

            let records: Vec<PriceRecord> = quotes
                .iter()
                .map(|q| {
                    let dt: DateTime<Utc> =
                        DateTime::from_timestamp(q.timestamp as i64, 0).unwrap_or_default();
                    let date = dt.format("%Y-%m-%d").to_string();

                    let change_usd = prev_close_usd.map(|prev| q.close - prev);
                    if let Some(c) = change_usd {
                        cumul_usd += c;
                    }
                    prev_close_usd = Some(q.close);

                    let cur_values: Vec<CurrencyValues> = currencies
                        .iter()
                        .map(|cur| {
                            let rate = if cur == "USD" {
                                1.0
                            } else {
                                let fx = all_rates.get(cur).unwrap();
                                lookup_rate(fx, &date, last_rates.get_mut(cur).unwrap())
                            };
                            let change = change_usd.map(|c| c * rate);
                            if let Some(c) = change {
                                *cumul_local.get_mut(cur).unwrap() += c;
                            }
                            CurrencyValues {
                                currency: cur.clone(),
                                close: q.close * rate,
                                change,
                                cumulative: if change.is_some() {
                                    Some(cumul_local[cur])
                                } else {
                                    None
                                },
                                exchange_rate: rate,
                            }
                        })
                        .collect();

                    PriceRecord {
                        date,
                        open: q.open,
                        high: q.high,
                        low: q.low,
                        close: q.close,
                        volume: q.volume,
                        change_usd,
                        cumulative_usd: Some(cumul_usd),
                        currencies: cur_values,
                    }
                })
                .collect();

            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else {
                // Print USD table
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

                // Print a table per currency
                for cur in &currencies {
                    if cur == "USD" {
                        continue;
                    }
                    println!();
                    println!(
                        "{:<12} {:>10} {:>8} {:>8} {:>11}",
                        "Date",
                        format!("Close({cur})"),
                        format!("Chg({cur})"),
                        format!("Cum({cur})"),
                        "Rate"
                    );
                    for r in &records {
                        let cv = r.currencies.iter().find(|v| v.currency == *cur).unwrap();
                        let chg = cv.change.map(format_change).unwrap_or_default();
                        let cum = cv.cumulative.map(format_change).unwrap_or_default();
                        println!(
                            "{:<12} {:>10.2} {:>8} {:>8} {:>11.4}",
                            r.date, cv.close, chg, cum, cv.exchange_rate
                        );
                    }
                }
            }
        }
        Command::Yield {
            symbol,
            currency,
            json,
            range,
        } => {
            let currencies = normalize_currencies(&currency);
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

            let all_rates = get_all_exchange_rates(&provider, &currencies, "1d", &range).await?;

            let start_str = start_date.format("%Y-%m-%d").to_string();
            let end_str = end_date.format("%Y-%m-%d").to_string();

            let currency_yields: Vec<CurrencyYield> = currencies
                .iter()
                .filter(|c| *c != "USD")
                .map(|cur| {
                    let fx = all_rates.get(cur).unwrap();
                    let mut lr = 1.0_f64;
                    let start_rate = lookup_rate(fx, &start_str, &mut lr);
                    let end_rate = lookup_rate(fx, &end_str, &mut lr);

                    let sp = start_price_usd * start_rate;
                    let ep = end_price_usd * end_rate;
                    let pc = ep - sp;

                    let mut div_local = 0.0_f64;
                    let mut dlr = 1.0_f64;
                    for d in &dividends {
                        let dt: DateTime<Utc> =
                            DateTime::from_timestamp(d.date, 0).unwrap_or_default();
                        let date = dt.format("%Y-%m-%d").to_string();
                        let rate = lookup_rate(fx, &date, &mut dlr);
                        div_local += d.amount.to_f64().unwrap_or(0.0) * rate;
                    }

                    let tr = pc + div_local;
                    let tr_pct = (tr / sp) * 100.0;
                    let ay_pct = annualize(tr_pct, days);

                    CurrencyYield {
                        currency: cur.clone(),
                        start_price: sp,
                        end_price: ep,
                        price_change: pc,
                        total_dividends: div_local,
                        total_return: tr,
                        total_return_pct: tr_pct,
                        annualized_yield_pct: ay_pct,
                    }
                })
                .collect();

            let report = YieldReport {
                symbol: symbol.clone(),
                range: range.clone(),
                days,
                start_date: start_str.clone(),
                end_date: end_str.clone(),
                start_price_usd,
                end_price_usd,
                price_change_usd,
                total_dividends_usd,
                total_return_usd,
                total_return_pct,
                annualized_yield_pct,
                currencies: currency_yields,
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

                for cy in &report.currencies {
                    println!();
                    println!("  {}:", cy.currency);
                    println!("    Start price:      {:>10.2}", cy.start_price);
                    println!("    End price:        {:>10.2}", cy.end_price);
                    println!("    Price change:     {:>10.2}", cy.price_change);
                    println!("    Dividends:        {:>10.2}", cy.total_dividends);
                    println!(
                        "    Total return:     {:>10.2} ({:+.2}%)",
                        cy.total_return, cy.total_return_pct
                    );
                    println!(
                        "    Annualized yield: {:>10}  ({:+.2}%)",
                        "", cy.annualized_yield_pct
                    );
                }
            }
        }
    }

    Ok(())
}
