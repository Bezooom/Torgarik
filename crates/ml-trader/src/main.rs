//! Unified binary:
//!   --mode shell     risk/journal shell (default money-safe path)
//!   --mode anomaly   structural anomaly logger
//!   --mode alpha     experimental auto strategy (OOS was red — research only)
//!
//! Legacy: cargo run -p ml-trader -- --mode dryrun  → treated as alpha dry-run.

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use trading_common::{ConfigStore, RunMode, Settings};
use trading_exchange::build_exchange_pair;
use trading_strategies::FundingScannerStrategy;

use ml_trader::anomaly::run_anomaly;
use ml_trader::risk_shell::run_shell;
use ml_trader::{default_strategy, Trader};

#[derive(Parser, Debug)]
#[command(
    name = "ml-trader",
    about = "Risk shell / anomaly logger / experimental alpha"
)]
struct Cli {
    /// shell | signals | anomaly | alpha | dryrun | live | paper
    #[arg(long, default_value = "shell")]
    mode: String,

    #[arg(long)]
    venue: Option<String>,

    /// For alpha only: majors_alpha | funding_scanner
    #[arg(long)]
    strategy: Option<String>,

    /// shell: actually send orders in live (still needs CONFIRM_LIVE + keys)
    #[arg(long, default_value_t = false)]
    execute: bool,

    /// signals: evaluate only this coin (default: exchange.symbol_whitelist)
    #[arg(long, default_value = "")]
    coin: String,
}

const BANNER: &str = r#"
 _____                            _ __  
|_   _|__  _ __ __ _  __ _  _ __ (_) | __
  | |/ _ \| '__/ _` |/ _` || '__|| | |/ /
  | | (_) | | | (_| | (_| || |   | |   < 
  |_|\___/|_|  \__,_|\__,_||_|   |_|_|\_\
--- Torgarik Risk Shell v1.0 [Hyperliquid] ---
"#;

#[tokio::main]
async fn main() -> Result<()> {
    println!("{BANNER}");
    let cli = Cli::parse();
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| {
        "ml_trader=info,trading_exchange=info,trading_strategies=info".into()
    });
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::new(filter))
        .init();

    let mode_l = cli.mode.to_ascii_lowercase();
    match mode_l.as_str() {
        "shell" | "risk" | "journal" => {
            info!("=== RISK / JOURNAL SHELL (no auto alpha) ===");
            run_shell(cli.execute).await?;
        }
        "anomaly" | "anomalies" | "hunt" => {
            info!("=== STRUCTURAL ANOMALY LOGGER ===");
            run_anomaly().await?;
        }
        "alpha" | "dryrun" | "paper" | "live" | "backtest" => {
            info!("=== EXPERIMENTAL ALPHA (research; OOS was not live-ready) ===");
            run_alpha(&cli, &mode_l).await?;
        }
        "tg" | "tg-bot" | "bot" => {
            info!("=== TELEGRAM CONTROL (status / stop / resume; never orders) ===");
            let cfg = ml_trader::tg_control::BotConfig::from_env().ok_or_else(|| {
                anyhow::anyhow!("set TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID to run the bot")
            })?;
            ml_trader::tg_control::run_bot(cfg).await?;
        }
        "signals" | "signal" => {
            info!("=== SIGNAL GENERATOR (shared trend plan → signals.jsonl) ===");
            run_signals_mode(&cli)?;
        }
        other => {
            anyhow::bail!(
                "unknown --mode {other}. Use: shell | signals | tg-bot | anomaly | alpha"
            );
        }
    }
    Ok(())
}

/// Compute the shared trend plan on recent history and append shell signals.
///
/// Never places orders: it only writes `runtime/signals.jsonl`, which the shell
/// re-validates through every gate. `--execute` is irrelevant here by design.
fn run_signals_mode(cli: &Cli) -> Result<()> {
    use ml_trader::signal_gen::{
        append_signal, history_path, latest_side, load_series, signal_line,
    };
    use trading_common::Settings;
    use trading_strategies::signals::TrendParams;

    let settings = Settings::load()?;
    let st = &settings.strategy;
    let params = TrendParams {
        ema_fast: st.ema_fast,
        ema_slow: st.ema_slow,
        min_trend_strength: st.min_trend_strength,
        funding_veto: st.funding_veto,
        vol_lookback: st.vol_lookback,
        min_vol_ratio: st.min_vol_ratio,
    };

    let coins: Vec<String> = if cli.coin.is_empty() {
        settings.exchange.symbol_whitelist.clone()
    } else {
        vec![cli.coin.to_uppercase()]
    };
    if coins.is_empty() {
        anyhow::bail!("no coins: set exchange.symbol_whitelist or pass --coin");
    }

    let out = std::path::PathBuf::from("runtime/signals.jsonl");
    let mut emitted = 0usize;
    for coin in &coins {
        let path = history_path(coin);
        let (_, mids, funding) = match load_series(&path) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("{coin}: {e} - skipped (run scripts/collect_hl_history.py)");
                continue;
            }
        };
        match latest_side(&mids, &funding, &params) {
            Some(side) => {
                let line = signal_line(coin, side, &params, mids.len());
                append_signal(&out, &line)?;
                emitted += 1;
                info!(
                    "{coin}: {} ({} bars, source=trend_plan)",
                    line.side, mids.len()
                );
            }
            None => info!("{coin}: flat ({} bars) — no signal", mids.len()),
        }
    }
    info!(
        "signals: {emitted}/{} coins emitted → {}",
        coins.len(),
        out.display()
    );
    Ok(())
}

async fn run_alpha(cli: &Cli, mode_l: &str) -> Result<()> {
    if let Some(venue) = &cli.venue {
        std::env::set_var("PRIMARY_VENUE", venue);
    }
    // map legacy mode names
    let run = match mode_l {
        "live" => {
            std::env::set_var("RUN_MODE", "live");
            "live"
        }
        "paper" => {
            std::env::set_var("RUN_MODE", "paper");
            "paper"
        }
        "backtest" => {
            std::env::set_var("RUN_MODE", "backtest");
            "backtest"
        }
        _ => {
            std::env::set_var("RUN_MODE", "dryrun");
            "dryrun"
        }
    };
    let _ = run;

    let mut settings = Settings::load()?;
    if mode_l == "live" {
        settings.mode = RunMode::Live;
    } else if mode_l == "paper" {
        settings.mode = RunMode::Paper;
    } else {
        settings.mode = RunMode::DryRun;
    }
    if let Some(venue) = &cli.venue {
        settings.exchange.primary_venue = venue.clone();
    }
    if let Some(name) = &cli.strategy {
        settings.strategy.name = name.clone();
    }
    if settings.strategy.name.is_empty() || settings.strategy.name == "funding_carry" {
        settings.strategy.name = "majors_alpha".into();
    }
    settings.validate()?;

    let bankroll = settings.risk.starting_bankroll;
    let mode = settings.mode;
    let strat_name = settings.strategy.name.clone();

    // Shared HL client: refresh + quotes + paper fills must not diverge.
    let (exchange, hl) = build_exchange_pair(&settings.exchange, mode, bankroll)?;
    let config = ConfigStore::from_settings(settings);
    let strategy = default_strategy(&config);

    info!(
        "alpha strategy={} (experimental) allowlist={:?}",
        strat_name, config.settings.exchange.symbol_whitelist
    );

    let _ = trading_common::metrics::init(&format!("0.0.0.0:{}", config.metrics_port));
    let mut trader = Trader::new(config.clone(), exchange, strategy).with_hl_quotes(hl.clone());
    if strat_name == "funding_scanner" {
        let scanner = FundingScannerStrategy::from_settings(
            &config.settings.strategy,
            &config.settings.exchange.symbol_whitelist,
            config.settings.risk.max_position_per_instrument,
            config.settings.risk.stop_loss,
        );
        trader = trader.with_scanner(hl, scanner);
    }
    trader.run().await?;
    Ok(())
}
