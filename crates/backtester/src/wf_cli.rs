//! Walk-forward CLI: end-to-end runner that loads history, runs WF,
//! writes `data/reports/wf_latest.json`, and optionally prints a summary.

use std::path::PathBuf;

use clap::Parser;

use crate::data::load_history_dir;
use crate::walk_forward::{
    run_walk_forward_opts, AlphaKind, FundingGrid, TrendGrid, WalkForwardReport, WfOptions,
};

/// Subcommand: run walk-forward analysis on a history directory.
#[derive(Parser, Debug, Clone)]
pub struct WalkForwardCli {
    /// Directory containing `COIN_1h.{json,csv}` files.
    #[arg(short, long)]
    pub data_dir: String,

    /// Starting capital per coin-fold.
    #[arg(short, long, default_value_t = 10_000.0)]
    pub capital: f64,

    /// Number of walk-forward folds.
    #[arg(short = 'n', long, default_value_t = 3)]
    pub n_folds: usize,

    /// Train fraction (0.5–0.9).
    #[arg(long, default_value_t = 0.7)]
    pub train_frac: f64,

    /// Alpha kind: "funding" or "trend".
    #[arg(short, long, default_value = "funding")]
    pub alpha: String,

    /// Holdout tail fraction (0 = off).
    #[arg(long, default_value_t = 0.15)]
    pub holdout_frac: f64,

    /// Embargo bars between train and test.
    #[arg(long, default_value_t = 2)]
    pub embargo: usize,

    /// Output report path (writes JSON).
    #[arg(short, long, default_value = "data/reports/wf_latest.json")]
    pub output: String,

    /// Print a human-readable summary to stdout as JSON.
    #[arg(long)]
    pub json: bool,
}

fn parse_alpha(s: &str) -> anyhow::Result<AlphaKind> {
    match s.to_ascii_lowercase().as_str() {
        "funding" => Ok(AlphaKind::Funding),
        "trend" => Ok(AlphaKind::Trend),
        other => anyhow::bail!("unknown alpha kind '{other}' (expected: funding|trend)"),
    }
}

/// Run the walk-forward pipeline from a CLI invocation and write the report.
pub fn run_walk_forward_cli(args: &WalkForwardCli) -> anyhow::Result<WalkForwardReport> {
    let alpha = parse_alpha(&args.alpha)?;

    let dir = PathBuf::from(&args.data_dir);
    let series = load_history_dir(&dir)?;
    let input: Vec<(String, Vec<crate::engine::Bar>)> =
        series.into_iter().map(|s| (s.coin, s.bars)).collect();

    let report = run_walk_forward_opts(
        &input,
        alpha,
        &FundingGrid::default(),
        &TrendGrid::default(),
        &WfOptions {
            capital: args.capital,
            n_folds: args.n_folds,
            train_frac: args.train_frac,
            expanding: true,
            embargo_bars: args.embargo,
            holdout_frac: args.holdout_frac,
        },
    );

    let out_path = PathBuf::from(&args.output);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Full report includes schema_version, generated_at, exec, holdout, realism_notes.
    let json = serde_json::to_string_pretty(&report)?;
    std::fs::write(&out_path, json)?;

    if args.json {
        let summary = serde_json::json!({
            "schema_version": report.schema_version,
            "generated_at": report.generated_at,
            "alpha": report.alpha,
            "n_coins": report.n_coins,
            "n_folds": report.n_folds,
            "test_total_pnl": report.test_total_pnl,
            "test_avg_sharpe": report.test_avg_sharpe,
            "test_avg_max_dd": report.test_avg_max_dd,
            "test_win_rate": report.test_win_rate,
            "profitable_test_folds": report.profitable_test_folds,
            "holdout": report.holdout.as_ref().map(|h| serde_json::json!({
                "total_pnl": h.total_pnl,
                "avg_sharpe": h.avg_sharpe,
                "n_coins": h.n_coins,
            })),
            "is_blocking": is_report_blocking(&report),
            "exec": report.exec,
        });
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        let holdout_s = report
            .holdout
            .as_ref()
            .map(|h| {
                format!(
                    " holdout_pnl={:.2} holdout_sharpe={:.3}",
                    h.total_pnl, h.avg_sharpe
                )
            })
            .unwrap_or_default();
        println!(
            "Walk-forward [{alpha}] {n_coins} coins × {n_folds} folds → \
             total_pnl={pnl:.2} avg_sharpe={sharpe:.3} win_rate={wr:.1}% \
             profitable_folds={prof}/{n_folds} kill_switch={blocking}{holdout}",
            alpha = report.alpha,
            n_coins = report.n_coins,
            n_folds = report.n_folds,
            pnl = report.test_total_pnl,
            sharpe = report.test_avg_sharpe,
            wr = report.test_win_rate * 100.0,
            prof = report.profitable_test_folds,
            blocking = is_report_blocking(&report),
            holdout = holdout_s,
        );
    }

    Ok(report)
}

/// Kill-switch: avg Sharpe < 0 OR total PnL < 0 (WF OOS). Holdout red is advisory in print.
fn is_report_blocking(r: &WalkForwardReport) -> bool {
    r.test_avg_sharpe < 0.0 || r.test_total_pnl < 0.0
}
