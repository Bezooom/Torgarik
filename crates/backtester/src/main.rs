//! Backtester CLI: funding-carry or majors_alpha walk-forward.

use std::path::PathBuf;

use backtester::{
    load_history_dir_as_of, load_series_as_of, run_walk_forward_opts, AlphaKind, BacktestEngine,
    Bar, FundingBacktestConfig, FundingGrid, TrendBacktestConfig, TrendGrid, WfOptions,
};
use clap::Parser;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "backtester", about = "Walk-forward: funding | majors_alpha")]
struct Cli {
    #[arg(long, default_value_t = false)]
    walk_forward: bool,

    /// funding | trend | majors_alpha
    #[arg(long, default_value = "trend")]
    alpha: String,

    #[arg(long, default_value = "data/history")]
    data_dir: PathBuf,

    #[arg(long)]
    file: Option<PathBuf>,

    #[arg(long, default_value = "")]
    coins: String,

    /// majors | core (BTC/ETH/SOL) | ethsol (ETH+SOL only) | meme
    #[arg(long, default_value = "ethsol")]
    preset: String,

    #[arg(long, default_value_t = 10_000.0)]
    capital: f64,

    #[arg(long, default_value_t = 3)]
    folds: usize,

    #[arg(long, default_value_t = 0.7)]
    train_frac: f64,

    #[arg(long, default_value_t = true)]
    expanding: bool,

    /// Gap bars between train end and test start (reduces leakage)
    #[arg(long, default_value_t = 2)]
    embargo: usize,

    /// Base slippage bps each side (default 2)
    #[arg(long, default_value_t = 2.0)]
    slip_bps: f64,

    /// Volume impact bps scaled by sqrt(participation) (default 1)
    #[arg(long, default_value_t = 1.0)]
    impact_bps: f64,

    /// Taker fee bps (HL default 3.5)
    #[arg(long, default_value_t = 3.5)]
    taker_bps: f64,

    /// Extra bars after next-bar fill (order queue / latency)
    #[arg(long, default_value_t = 0)]
    queue_delay: usize,

    /// Fraction of each series tail reserved as pure holdout (0–0.4, default 0.15)
    #[arg(long, default_value_t = 0.15)]
    holdout_frac: f64,

    /// Min mean |hourly ret| for vol regime (0 = off)
    #[arg(long, default_value_t = 0.0025)]
    min_vol: f64,

    #[arg(long)]
    enter: Option<f64>,

    #[arg(long)]
    report: Option<PathBuf>,

    /// Point-in-time cut: replay history as the world looked at this instant.
    /// Accepts unix seconds or `YYYY-MM-DD` (UTC midnight).
    #[arg(long)]
    as_of: Option<String>,

    /// Skip the look-ahead (causality) audit. Only for deliberately leaky research.
    #[arg(long, default_value_t = false)]
    no_audit: bool,

    /// Print the pre-registration hash of this hypothesis + data and exit,
    /// without running the backtest. Commit the hash before you look at results.
    #[arg(long, default_value_t = false)]
    prereg_only: bool,

    /// Require the run to reproduce this pre-registration hash (exit 4 on drift).
    #[arg(long)]
    prereg_expect: Option<String>,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "backtester=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let alpha = match cli.alpha.to_ascii_lowercase().as_str() {
        "funding" | "funding_carry" | "carry" => AlphaKind::Funding,
        _ => AlphaKind::Trend,
    };

    let as_of = match cli.as_of.as_deref().map(parse_as_of) {
        Some(Ok(ts)) => {
            info!("point-in-time as_of={ts} ({} UTC)", fmt_utc(ts));
            Some(ts)
        }
        Some(Err(e)) => {
            eprintln!("--as-of: {e}");
            std::process::exit(2);
        }
        None => None,
    };

    if !cli.walk_forward && cli.file.is_none() {
        info!("=== Synthetic demos (use --walk-forward for real data) ===");
        let engine = BacktestEngine::new(cli.capital);
        // funding synth
        let mut bars = Vec::new();
        let mut mid = 100.0;
        for i in 0..200 {
            let funding = if (i / 40) % 2 == 0 { 0.00015 } else { -0.00012 };
            mid *= 1.0 + (((i * 17) % 7) as f64 - 3.0) * 0.0003;
            bars.push(Bar::new(i as i64 * 3600, mid, funding));
        }
        print_result(
            "funding_synth",
            &engine.run_funding(&bars, &FundingBacktestConfig::default()),
        );
        // trend synth
        mid = 100.0;
        bars.clear();
        for i in 0..200 {
            mid *= if (i / 50) % 2 == 0 { 1.002 } else { 0.998 };
            bars.push(Bar::new(i as i64 * 3600, mid, 0.0));
        }
        print_result(
            "trend_synth",
            &engine.run_trend(&bars, &TrendBacktestConfig::default()),
        );
        info!("Real: cargo run -p backtester -- --walk-forward --alpha trend --preset majors");
        return;
    }

    let series_list = if let Some(file) = &cli.file {
        match load_series_as_of(file, as_of) {
            Ok(s) => vec![s],
            Err(e) => {
                eprintln!("load {}: {e}", file.display());
                std::process::exit(1);
            }
        }
    } else {
        match load_history_dir_as_of(&cli.data_dir, as_of) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    };

    let mut filter: Vec<String> = cli
        .coins
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    if filter.is_empty() {
        match cli.preset.to_ascii_lowercase().as_str() {
            "majors" | "b" => {
                filter = vec![
                    "BTC", "ETH", "SOL", "XRP", "BNB", "LINK", "AVAX", "DOGE", "ARB", "SUI", "LTC",
                    "UNI", "AAVE", "TRX", "DOT", "ATOM", "NEAR", "APT", "OP", "HYPE",
                ]
                .into_iter()
                .map(String::from)
                .collect();
                info!("preset=majors → {} coins", filter.len());
            }
            "meme" => {
                filter = vec![
                    "ACE", "NOT", "MOVE", "SUPER", "ALT", "kNEIRO", "JUP", "WLD", "ZK",
                ]
                .into_iter()
                .map(String::from)
                .collect();
                info!("preset=meme");
            }
            "core" => {
                filter = vec!["BTC", "ETH", "SOL"]
                    .into_iter()
                    .map(String::from)
                    .collect();
                info!("preset=core → BTC,ETH,SOL");
            }
            "ethsol" | "es" | "" => {
                filter = vec!["ETH", "SOL"].into_iter().map(String::from).collect();
                info!("preset=ethsol → ETH,SOL only (vol-regime focus)");
            }
            _ => {}
        }
    }

    let series: Vec<(String, Vec<backtester::Bar>)> = series_list
        .into_iter()
        .filter(|s| filter.is_empty() || filter.iter().any(|c| c == &s.coin))
        .map(|s| {
            info!("loaded {} bars={}", s.coin, s.bars.len());
            (s.coin, s.bars)
        })
        .collect();

    if series.is_empty() {
        eprintln!("no series after filter");
        std::process::exit(1);
    }

    // Look-ahead audit: every signal generator must produce an identical prefix
    // when it cannot see the future. Runs before any PnL is reported.
    if !cli.no_audit {
        let engine = BacktestEngine::new(cli.capital);
        let mut failures = 0usize;
        let mut audited = 0usize;
        for (coin, bars) in &series {
            let outcome = match alpha {
                AlphaKind::Funding => {
                    let cfg = FundingBacktestConfig::default();
                    BacktestEngine::audit_causality(bars, 8, |b| engine.funding_plan(b, &cfg))
                }
                AlphaKind::Trend => {
                    let mut cfg = TrendBacktestConfig::default();
                    cfg.min_vol_ratio = cli.min_vol;
                    BacktestEngine::audit_causality(bars, 8, |b| engine.trend_plan(b, &cfg))
                }
            };
            match outcome {
                Ok(probes) => {
                    audited += 1;
                    info!("look-ahead audit {coin}: {probes} probes clean");
                }
                Err(errors) => {
                    failures += 1;
                    for e in errors.iter().take(3) {
                        eprintln!("LOOK-AHEAD {coin}: {e}");
                    }
                }
            }
        }
        if failures > 0 {
            eprintln!(
                "look-ahead audit FAILED on {failures}/{audited} series – refusing to report PnL"
            );
            std::process::exit(3);
        }
        info!("look-ahead audit: {audited} series clean");
    }

    if let Some(enter) = cli.enter {
        info!("=== Full-sample fixed (funding enter={enter}) ===");
        let engine = BacktestEngine::new(cli.capital);
        let mut cfg = FundingBacktestConfig::default();
        cfg.enter = enter;
        cfg.exit = enter * 0.35;
        cfg.exec.slip_bps = cli.slip_bps;
        cfg.exec.next_bar = true;
        for (coin, bars) in &series {
            print_result(coin, &engine.run_funding(bars, &cfg));
        }
        return;
    }

    info!(
        "=== Walk-forward alpha={:?} folds={} capital={} embargo={} holdout={} slip={} impact={} taker={} queue={} min_vol={} ===",
        alpha, cli.folds, cli.capital, cli.embargo, cli.holdout_frac,
        cli.slip_bps, cli.impact_bps, cli.taker_bps, cli.queue_delay, cli.min_vol
    );

    let mut funding_grid = FundingGrid::default();
    funding_grid.exec.slip_bps = cli.slip_bps;
    funding_grid.exec.impact_bps = cli.impact_bps;
    funding_grid.exec.taker_bps = cli.taker_bps;
    funding_grid.exec.queue_delay_bars = cli.queue_delay;
    let mut trend_grid = TrendGrid::default();
    trend_grid.exec.slip_bps = cli.slip_bps;
    trend_grid.exec.impact_bps = cli.impact_bps;
    trend_grid.exec.taker_bps = cli.taker_bps;
    trend_grid.exec.queue_delay_bars = cli.queue_delay;
    trend_grid.min_vol_ratio = cli.min_vol;

    let wf_opts = WfOptions {
        capital: cli.capital,
        n_folds: cli.folds,
        train_frac: cli.train_frac,
        expanding: cli.expanding,
        embargo_bars: cli.embargo,
        holdout_frac: cli.holdout_frac,
    };

    if cli.prereg_only {
        let desc =
            backtester::prereg::descriptor(&series, alpha, &funding_grid, &trend_grid, &wf_opts);
        let hash = backtester::prereg::hash(&desc);
        println!("{hash}");
        eprintln!(
            "prereg: {} series, {} folds, holdout={} - commit this hash, then run with --prereg-expect {hash}",
            desc.data.len(),
            desc.n_folds,
            desc.holdout_frac
        );
        return;
    }

    let report = run_walk_forward_opts(&series, alpha, &funding_grid, &trend_grid, &wf_opts);

    for f in &report.folds {
        info!(
            "[{}] fold={} {} train_pnl={:.2} TEST_pnl={:.2} sharpe={:.2} dd={:.1}%",
            f.coin,
            f.fold,
            f.best_label,
            f.train.total_pnl,
            f.test.total_pnl,
            f.test.sharpe_ratio,
            f.test.max_drawdown * 100.0,
        );
    }

    info!("-------- OOS SUMMARY alpha={} --------", report.alpha);
    info!(
        "coins={} folds_total={}",
        report.n_coins,
        report.folds.len()
    );
    info!("test_total_pnl: ${:.2}", report.test_total_pnl);
    info!("test_avg_sharpe: {:.2}", report.test_avg_sharpe);
    info!("test_avg_max_dd: {:.1}%", report.test_avg_max_dd * 100.0);
    info!("test_win_rate: {:.1}%", report.test_win_rate * 100.0);
    info!(
        "profitable_test_folds: {}/{}",
        report.profitable_test_folds,
        report.folds.len()
    );
    info!("n_trials: {}", report.n_trials);
    if report.deflated_sharpe_min.is_finite() {
        info!(
            "deflated_sharpe: min={:.3} avg={:.3} (selection-bias corrected)",
            report.deflated_sharpe_min, report.deflated_sharpe_avg
        );
    }
    if let Some(pbo) = report.pbo_avg {
        info!("pbo: {:.3} (probability of backtest overfitting)", pbo);
    }
    info!("prereg_hash: {}", report.prereg_hash);

    if let Some(expected) = cli.prereg_expect.as_deref() {
        if let Err(e) = backtester::prereg::verify(expected, &report.prereg_hash) {
            eprintln!("{e}");
            std::process::exit(4);
        }
        info!("prereg: hash reproduced");
    }

    // Mode of best_label
    let mut labels: Vec<String> = report.folds.iter().map(|f| f.best_label.clone()).collect();
    labels.sort();
    if let Some(mode_label) = labels
        .windows(1)
        .map(|w| w[0].clone())
        .max_by_key(|l| labels.iter().filter(|x| *x == l).count())
    {
        info!("most common train pick: {mode_label}  → config ema_fast/ema_slow or funding_enter");
    }

    if let Some(h) = &report.holdout {
        info!(
            "HOLDOUT frac={} coins={} total_pnl={:.2} avg_sharpe={:.2} profitable={}/{}",
            h.holdout_frac, h.n_coins, h.total_pnl, h.avg_sharpe, h.profitable_coins, h.n_coins
        );
    }

    if let Some(dsr) = report
        .deflated_sharpe_min
        .is_finite()
        .then_some(report.deflated_sharpe_min)
    {
        if dsr <= 0.0 {
            info!("VERDICT: deflated Sharpe ≤ 0 – winner is indistinguishable from luck");
        }
    }
    if report.pbo_avg.map(|p| p > 0.5).unwrap_or(false) {
        info!("VERDICT: PBO > 0.5 – in-sample winner does not survive out of sample");
    }

    if report.test_total_pnl <= 0.0 {
        info!("VERDICT: OOS PnL ≤ 0 – do NOT go live");
    } else if report.profitable_test_folds * 2 < report.folds.len() {
        info!("VERDICT: OOS > 0 but minority green folds – fragile");
    } else if report
        .holdout
        .as_ref()
        .map(|h| h.total_pnl <= 0.0 || h.avg_sharpe < 0.0)
        .unwrap_or(false)
    {
        info!("VERDICT: WF OOS ok-ish but HOLDOUT red – do NOT unlock live");
    } else {
        info!("VERDICT: OOS positive majority – paper before any live");
    }

    if let Some(path) = cli.report {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            &path,
            serde_json::to_string_pretty(&report).unwrap_or_default(),
        );
        info!(
            "report → {} (schema {})",
            path.display(),
            report.schema_version
        );
    }
}

fn print_result(label: &str, r: &backtester::BacktestResult) {
    info!(
        "[{}] pnl={:.2} fund={:.2} price={:.2} fees={:.2} trades={} win={:.0}% sharpe={:.2} dd={:.1}%",
        label,
        r.total_pnl,
        r.funding_pnl,
        r.price_pnl,
        r.fees_paid,
        r.total_trades,
        r.win_rate * 100.0,
        r.sharpe_ratio,
        r.max_drawdown * 100.0
    );
}

/// Parse `--as-of`: unix seconds or `YYYY-MM-DD` (UTC midnight).
fn parse_as_of(raw: &str) -> Result<i64, String> {
    let raw = raw.trim();
    if let Ok(ts) = raw.parse::<i64>() {
        if ts <= 0 {
            return Err(format!("{ts} is not a positive unix timestamp"));
        }
        return Ok(ts);
    }
    let date = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|e| format!("{raw:?} is neither unix seconds nor YYYY-MM-DD ({e})"))?;
    Ok(date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| format!("{raw:?} has no midnight"))?
        .and_utc()
        .timestamp())
}

/// Human-readable UTC stamp for logs.
fn fmt_utc(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "invalid".into())
}
