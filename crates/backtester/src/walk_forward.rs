//! Walk-forward: funding or trend with next-bar/slip configs + train/test embargo + holdout.

use serde::Serialize;

use crate::engine::{
    BacktestEngine, BacktestResult, ExecConfig, FundingBacktestConfig, TrendBacktestConfig,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaKind {
    Funding,
    Trend,
}

#[derive(Debug, Clone, Serialize)]
pub struct FoldResult {
    pub coin: String,
    pub fold: usize,
    pub train_bars: usize,
    pub test_bars: usize,
    pub best_param: f64,
    pub best_label: String,
    pub train: BacktestResult,
    pub test: BacktestResult,
    /// Number of grid trials the selection step evaluated for this fold.
    #[serde(default)]
    pub n_trials: usize,
    /// Deflated Sharpe of the test run: probability the winner is real after
    /// accounting for `n_trials` attempts. `NaN` when it cannot be computed.
    #[serde(default)]
    pub deflated_sharpe: f64,
    /// Probability of backtest overfitting (CSCV on the train window).
    #[serde(default)]
    pub pbo: Option<f64>,
}

/// Pure holdout evaluation (params fixed from walk-forward; never used in grid search).
#[derive(Debug, Clone, Serialize)]
pub struct HoldoutCoinResult {
    pub coin: String,
    pub bars: usize,
    pub param: f64,
    pub label: String,
    pub result: BacktestResult,
}

#[derive(Debug, Clone, Serialize)]
pub struct HoldoutSummary {
    pub holdout_frac: f64,
    pub n_coins: usize,
    pub total_pnl: f64,
    pub avg_sharpe: f64,
    pub avg_max_dd: f64,
    pub profitable_coins: usize,
    pub coins: Vec<HoldoutCoinResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowMeta {
    pub n_folds: usize,
    pub train_frac: f64,
    pub expanding: bool,
    pub embargo_bars: usize,
    pub holdout_frac: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WalkForwardReport {
    pub schema_version: String,
    pub generated_at: String,
    pub alpha: String,
    pub strategy: String,
    pub initial_capital: f64,
    pub n_coins: usize,
    pub n_folds: usize,
    pub embargo_bars: usize,
    pub holdout_frac: f64,
    pub window: WindowMeta,
    pub exec: ExecConfig,
    pub folds: Vec<FoldResult>,
    pub test_total_pnl: f64,
    pub test_avg_sharpe: f64,
    pub test_avg_max_dd: f64,
    pub test_win_rate: f64,
    pub profitable_test_folds: usize,
    /// Worst fold's deflated Sharpe (`NaN` if none computable).
    #[serde(default)]
    pub deflated_sharpe_min: f64,
    /// Mean fold deflated Sharpe.
    #[serde(default)]
    pub deflated_sharpe_avg: f64,
    /// Mean probability of backtest overfitting across folds with a verdict.
    #[serde(default)]
    pub pbo_avg: Option<f64>,
    /// Grid size actually searched per fold.
    #[serde(default)]
    pub n_trials: usize,
    /// Pre-registration hash of the hypothesis + data fingerprint.
    #[serde(default)]
    pub prereg_hash: String,
    /// Schema version bump marker for readers that care.
    #[serde(default)]
    pub prereg: Option<crate::prereg::PreregDescriptor>,
    pub holdout: Option<HoldoutSummary>,
    /// Protocol note for sim vs paper comparison.
    pub realism_notes: Vec<String>,
}

pub struct FundingGrid {
    pub enter_grid: Vec<f64>,
    pub exit_ratio: f64,
    pub exec: ExecConfig,
}

impl Default for FundingGrid {
    fn default() -> Self {
        Self {
            enter_grid: vec![
                0.00002, 0.00003, 0.00004, 0.00005, 0.00008, 0.00012, 0.00020,
            ],
            exit_ratio: 0.35,
            exec: ExecConfig::default(),
        }
    }
}

pub type GridSpec = FundingGrid;

pub struct TrendGrid {
    pub pairs: Vec<(usize, usize)>,
    pub funding_veto: f64,
    pub min_trend_strength: f64,
    pub vol_lookback: usize,
    pub min_vol_ratio: f64,
    pub exec: ExecConfig,
}

impl Default for TrendGrid {
    fn default() -> Self {
        let mut pairs = Vec::new();
        for &fast in &[5usize, 8, 12] {
            for &slow in &[16usize, 21, 34] {
                if fast < slow {
                    pairs.push((fast, slow));
                }
            }
        }
        Self {
            pairs,
            funding_veto: 0.00015,
            min_trend_strength: 0.0008,
            vol_lookback: 24,
            min_vol_ratio: 0.0025,
            exec: ExecConfig::default(),
        }
    }
}

fn funding_cfg(enter: f64, g: &FundingGrid) -> FundingBacktestConfig {
    FundingBacktestConfig {
        enter,
        exit: enter * g.exit_ratio,
        exec: g.exec.clone(),
    }
}

fn trend_cfg(fast: usize, slow: usize, g: &TrendGrid) -> TrendBacktestConfig {
    TrendBacktestConfig {
        ema_fast: fast,
        ema_slow: slow,
        funding_veto: g.funding_veto,
        min_trend_strength: g.min_trend_strength,
        vol_lookback: g.vol_lookback,
        min_vol_ratio: g.min_vol_ratio,
        exec: g.exec.clone(),
    }
}

fn pick_funding(
    engine: &BacktestEngine,
    train: &[crate::engine::Bar],
    grid: &FundingGrid,
) -> (f64, String, BacktestResult) {
    let mut best_e = grid.enter_grid[0];
    let mut best = engine.run_funding(train, &funding_cfg(best_e, grid));
    for &enter in &grid.enter_grid {
        let r = engine.run_funding(train, &funding_cfg(enter, grid));
        if r.total_pnl > best.total_pnl + 1e-9
            || ((r.total_pnl - best.total_pnl).abs() < 1e-9 && r.sharpe_ratio > best.sharpe_ratio)
        {
            best = r;
            best_e = enter;
        }
    }
    (best_e, format!("enter={best_e:.6}"), best)
}

fn pick_trend(
    engine: &BacktestEngine,
    train: &[crate::engine::Bar],
    grid: &TrendGrid,
) -> (f64, String, BacktestResult) {
    let (mut bf, mut bs) = grid.pairs[0];
    let mut best = engine.run_trend(train, &trend_cfg(bf, bs, grid));
    for &(fast, slow) in &grid.pairs {
        let r = engine.run_trend(train, &trend_cfg(fast, slow, grid));
        if r.total_pnl > best.total_pnl + 1e-9
            || ((r.total_pnl - best.total_pnl).abs() < 1e-9 && r.sharpe_ratio > best.sharpe_ratio)
        {
            best = r;
            bf = fast;
            bs = slow;
        }
    }
    (bf as f64 + (bs as f64) / 1000.0, format!("ema={bf}/{bs}"), best)
}

/// Pick funding enter by out-of-sample inner validation: optimize on a held-out
/// tail of the train window rather than the raw train PnL. This avoids selecting
/// params that merely maximize in-sample noise. Falls back to the in-sample pick
/// when the train window is too short for a meaningful inner split.
fn pick_funding_cv(
    engine: &BacktestEngine,
    train: &[crate::engine::Bar],
    grid: &FundingGrid,
) -> (f64, String, BacktestResult) {
    let split = (train.len() as f64 * 0.8).round() as usize;
    let split = split.clamp(20, train.len());
    let inner_train = &train[..split];
    let inner_val = &train[split..];
    if inner_val.len() < 20 {
        return pick_funding(engine, train, grid);
    }
    let mut best_enter = grid.enter_grid[0];
    let (mut best_train, mut best_val_pnl) = {
        let e = grid.enter_grid[0];
        (
            engine.run_funding(inner_train, &funding_cfg(e, grid)),
            engine.run_funding(inner_val, &funding_cfg(e, grid)).total_pnl,
        )
    };
    for &enter in &grid.enter_grid {
        let train_res = engine.run_funding(inner_train, &funding_cfg(enter, grid));
        let val_res = engine.run_funding(inner_val, &funding_cfg(enter, grid));
        if val_res.total_pnl > best_val_pnl + 1e-9
            || ((val_res.total_pnl - best_val_pnl).abs() < 1e-9 && train_res.total_pnl > best_train.total_pnl)
        {
            best_val_pnl = val_res.total_pnl;
            best_train = train_res;
            best_enter = enter;
        }
    }
    (best_enter, format!("enter={best_enter:.6}"), best_train)
}

/// Pick trend EMA pair by out-of-sample inner validation (see `pick_funding_cv`).
fn pick_trend_cv(
    engine: &BacktestEngine,
    train: &[crate::engine::Bar],
    grid: &TrendGrid,
) -> (f64, String, BacktestResult) {
    let split = (train.len() as f64 * 0.8).round() as usize;
    let split = split.clamp(20, train.len());
    let inner_train = &train[..split];
    let inner_val = &train[split..];
    if inner_val.len() < 20 {
        return pick_trend(engine, train, grid);
    }
    let mut best_pair = grid.pairs[0];
    let (mut best_train, mut best_val_pnl) = {
        let (f, s) = grid.pairs[0];
        (
            engine.run_trend(inner_train, &trend_cfg(f, s, grid)),
            engine.run_trend(inner_val, &trend_cfg(f, s, grid)).total_pnl,
        )
    };
    for &(fast, slow) in &grid.pairs {
        let train_res = engine.run_trend(inner_train, &trend_cfg(fast, slow, grid));
        let val_res = engine.run_trend(inner_val, &trend_cfg(fast, slow, grid));
        if val_res.total_pnl > best_val_pnl + 1e-9
            || ((val_res.total_pnl - best_val_pnl).abs() < 1e-9 && train_res.total_pnl > best_train.total_pnl)
        {
            best_val_pnl = val_res.total_pnl;
            best_train = train_res;
            best_pair = (fast, slow);
        }
    }
    (
        best_pair.0 as f64 + (best_pair.1 as f64) / 1000.0,
        format!("ema={}/{}", best_pair.0, best_pair.1),
        best_train,
    )
}

/// What the selection step saw, kept so selection bias can be measured.
///
/// `matrix[row][trial]` is the per-bar return of trial `trial` at observation
/// `row` on the selection window; all trials share the same bars, so rows line
/// up. Only in memory: it never reaches the JSON report.
#[derive(Debug, Default)]
struct TrialScan {
    trial_sharpes: Vec<f64>,
    matrix: Vec<Vec<f64>>,
}

impl TrialScan {
    fn n_trials(&self) -> usize {
        self.trial_sharpes.len()
    }

    fn var_sr(&self) -> f64 {
        crate::stats::sharpe_variance(&self.trial_sharpes)
    }

    fn pbo(&self) -> Option<f64> {
        crate::stats::cscv_pbo(&self.matrix, 10)
    }
}

/// Re-run every grid trial on `window` and record what selection ignored:
/// the dispersion of trial Sharpes and the full T x N return matrix.
fn scan_trials(
    engine: &BacktestEngine,
    window: &[crate::engine::Bar],
    alpha: AlphaKind,
    funding_grid: &FundingGrid,
    trend_grid: &TrendGrid,
) -> TrialScan {
    let runs: Vec<BacktestResult> = match alpha {
        AlphaKind::Funding => funding_grid
            .enter_grid
            .iter()
            .map(|&e| engine.run_funding(window, &funding_cfg(e, funding_grid)))
            .collect(),
        AlphaKind::Trend => trend_grid
            .pairs
            .iter()
            .map(|&(f, s)| engine.run_trend(window, &trend_cfg(f, s, trend_grid)))
            .collect(),
    };
    if runs.is_empty() {
        return TrialScan::default();
    }
    let trial_sharpes: Vec<f64> = runs
        .iter()
        .map(|r| crate::stats::sharpe_per_obs(&r.bar_returns))
        .collect();
    // Align by the shortest series so a trial that skipped a bad bar cannot
    // shift the matrix columns.
    let rows = runs.iter().map(|r| r.bar_returns.len()).min().unwrap_or(0);
    let mut matrix = Vec::with_capacity(rows);
    for row in 0..rows {
        matrix.push(runs.iter().map(|r| r.bar_returns[row]).collect());
    }
    TrialScan {
        trial_sharpes,
        matrix,
    }
}

/// Deflated Sharpe of a run given the trial dispersion it was selected from.
fn dsr_of(result: &BacktestResult, scan: &TrialScan) -> f64 {
    let returns = &result.bar_returns;
    if returns.len() < 4 {
        return f64::NAN;
    }
    crate::stats::deflated_sharpe(
        crate::stats::sharpe_per_obs(returns),
        returns.len(),
        crate::stats::skewness(returns),
        crate::stats::kurtosis(returns),
        scan.var_sr(),
        scan.n_trials().max(1),
    )
}

fn run_with_param(
    engine: &BacktestEngine,
    bars: &[crate::engine::Bar],
    alpha: AlphaKind,
    param: f64,
    funding_grid: &FundingGrid,
    trend_grid: &TrendGrid,
) -> BacktestResult {
    match alpha {
        AlphaKind::Funding => engine.run_funding(bars, &funding_cfg(param, funding_grid)),
        AlphaKind::Trend => {
            let fast = param.floor() as usize;
            let slow = ((param - fast as f64) * 1000.0).round() as usize;
            engine.run_trend(bars, &trend_cfg(fast, slow.max(fast + 1), trend_grid))
        }
    }
}

/// Median of params (funding) or last-fold param for trend encoding.
fn aggregate_param(folds: &[FoldResult], alpha: AlphaKind) -> (f64, String) {
    if folds.is_empty() {
        return (0.0, "none".into());
    }
    match alpha {
        AlphaKind::Funding => {
            let mut ps: Vec<f64> = folds.iter().map(|f| f.best_param).collect();
            ps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let med = ps[ps.len() / 2];
            (med, format!("enter={med:.6}"))
        }
        AlphaKind::Trend => {
            // Mode of label; param of first fold with that label
            let mut labels: Vec<&str> = folds.iter().map(|f| f.best_label.as_str()).collect();
            labels.sort();
            let mut best_l = labels[0];
            let mut best_c = 0usize;
            let mut cur = labels[0];
            let mut c = 0usize;
            for l in &labels {
                if *l == cur {
                    c += 1;
                } else {
                    if c > best_c {
                        best_c = c;
                        best_l = cur;
                    }
                    cur = l;
                    c = 1;
                }
            }
            if c > best_c {
                best_l = cur;
            }
            let param = folds
                .iter()
                .find(|f| f.best_label == best_l)
                .map(|f| f.best_param)
                .unwrap_or(folds[0].best_param);
            (param, best_l.to_string())
        }
    }
}

pub fn walk_forward_coin(
    coin: &str,
    bars: &[crate::engine::Bar],
    capital: f64,
    n_folds: usize,
    train_frac: f64,
    alpha: AlphaKind,
    funding_grid: &FundingGrid,
    trend_grid: &TrendGrid,
    expanding: bool,
    embargo_bars: usize,
) -> Vec<FoldResult> {
    walk_forward_coin_main(
        coin,
        bars,
        capital,
        n_folds,
        train_frac,
        alpha,
        funding_grid,
        trend_grid,
        expanding,
        embargo_bars,
    )
}

fn walk_forward_coin_main(
    coin: &str,
    bars: &[crate::engine::Bar],
    capital: f64,
    n_folds: usize,
    train_frac: f64,
    alpha: AlphaKind,
    funding_grid: &FundingGrid,
    trend_grid: &TrendGrid,
    expanding: bool,
    embargo_bars: usize,
) -> Vec<FoldResult> {
    let n = bars.len();
    if n < 40 || n_folds == 0 {
        return vec![];
    }
    let engine = BacktestEngine::new(capital);
    let train_frac = train_frac.clamp(0.5, 0.9);
    let mut folds = Vec::new();
    let emb = embargo_bars;

    let run_fold = |train: &[crate::engine::Bar], test: &[crate::engine::Bar], fold: usize| {
        let (param, label, train_res) = match alpha {
            AlphaKind::Funding => pick_funding_cv(&engine, train, funding_grid),
            AlphaKind::Trend => pick_trend_cv(&engine, train, trend_grid),
        };
        let test_res = run_with_param(&engine, test, alpha, param, funding_grid, trend_grid);
        let scan = scan_trials(&engine, train, alpha, funding_grid, trend_grid);
        FoldResult {
            coin: coin.to_string(),
            fold,
            train_bars: train.len(),
            test_bars: test.len(),
            best_param: param,
            best_label: label,
            n_trials: scan.n_trials(),
            deflated_sharpe: dsr_of(&test_res, &scan),
            pbo: scan.pbo(),
            train: train_res,
            test: test_res,
        }
    };

    let fold_len = n / (n_folds + 1);
    if fold_len < 20 {
        let split = ((n as f64) * train_frac) as usize;
        if split < 15 || n - split < 15 {
            return vec![];
        }
        let test_start = (split + emb).min(n);
        if n - test_start < 10 {
            return vec![];
        }
        folds.push(run_fold(&bars[..split], &bars[test_start..], 0));
        return folds;
    }

    for f in 0..n_folds {
        let test_start_raw = fold_len * (f + 1);
        let test_end = if f + 1 == n_folds {
            n
        } else {
            fold_len * (f + 2)
        };
        let train_start = if expanding { 0 } else { fold_len * f };
        let train_end = test_start_raw;
        let test_start = (test_start_raw + emb).min(test_end);
        if train_end <= train_start + 15 || test_end <= test_start + 10 {
            continue;
        }
        folds.push(run_fold(
            &bars[train_start..train_end],
            &bars[test_start..test_end],
            f,
        ));
    }
    folds
}

/// Options for a full walk-forward + optional holdout run.
#[derive(Debug, Clone)]
pub struct WfOptions {
    pub capital: f64,
    pub n_folds: usize,
    pub train_frac: f64,
    pub expanding: bool,
    pub embargo_bars: usize,
    /// Fraction of each series' **tail** reserved as pure holdout (0 = off).
    pub holdout_frac: f64,
}

impl Default for WfOptions {
    fn default() -> Self {
        Self {
            capital: 10_000.0,
            n_folds: 3,
            train_frac: 0.7,
            expanding: true,
            embargo_bars: 2,
            holdout_frac: 0.15,
        }
    }
}

pub fn run_walk_forward(
    series: &[(String, Vec<crate::engine::Bar>)],
    capital: f64,
    n_folds: usize,
    train_frac: f64,
    alpha: AlphaKind,
    funding_grid: &FundingGrid,
    trend_grid: &TrendGrid,
    expanding: bool,
    embargo_bars: usize,
) -> WalkForwardReport {
    run_walk_forward_opts(
        series,
        alpha,
        funding_grid,
        trend_grid,
        &WfOptions {
            capital,
            n_folds,
            train_frac,
            expanding,
            embargo_bars,
            holdout_frac: 0.0,
        },
    )
}

pub fn run_walk_forward_opts(
    series: &[(String, Vec<crate::engine::Bar>)],
    alpha: AlphaKind,
    funding_grid: &FundingGrid,
    trend_grid: &TrendGrid,
    opts: &WfOptions,
) -> WalkForwardReport {
    let holdout_frac = opts.holdout_frac.clamp(0.0, 0.4);
    let mut folds = Vec::new();
    let mut holdout_coins = Vec::new();
    let engine = BacktestEngine::new(opts.capital);

    for (coin, bars) in series {
        let n = bars.len();
        let mut holdout_n = ((n as f64) * holdout_frac).floor() as usize;
        // Need enough bars for holdout evaluation
        if holdout_n > 0 && (holdout_n < 24 || n - holdout_n < 40) {
            holdout_n = 0;
        }
        let main_end = n - holdout_n;
        let main = &bars[..main_end];
        let coin_folds = walk_forward_coin_main(
            coin,
            main,
            opts.capital,
            opts.n_folds,
            opts.train_frac,
            alpha,
            funding_grid,
            trend_grid,
            opts.expanding,
            opts.embargo_bars,
        );

        if holdout_n >= 24 && !coin_folds.is_empty() {
            let (param, label) = aggregate_param(&coin_folds, alpha);
            let holdout_bars = &bars[main_end..];
            let result = run_with_param(
                &engine,
                holdout_bars,
                alpha,
                param,
                funding_grid,
                trend_grid,
            );
            holdout_coins.push(HoldoutCoinResult {
                coin: coin.clone(),
                bars: holdout_bars.len(),
                param,
                label,
                result,
            });
        }

        folds.extend(coin_folds);
    }

    let n = folds.len().max(1);
    let test_total_pnl: f64 = folds.iter().map(|f| f.test.total_pnl).sum();
    let test_avg_sharpe: f64 = folds.iter().map(|f| f.test.sharpe_ratio).sum::<f64>() / n as f64;
    let test_avg_max_dd: f64 = folds.iter().map(|f| f.test.max_drawdown).sum::<f64>() / n as f64;
    let test_wins: usize = folds.iter().map(|f| f.test.winning_trades).sum();
    let test_losses: usize = folds.iter().map(|f| f.test.losing_trades).sum();
    let tw = test_wins + test_losses;
    let test_win_rate = if tw > 0 {
        test_wins as f64 / tw as f64
    } else {
        0.0
    };
    let profitable_test_folds = folds.iter().filter(|f| f.test.total_pnl > 0.0).count();

    let dsrs: Vec<f64> = folds
        .iter()
        .map(|f| f.deflated_sharpe)
        .filter(|d| d.is_finite())
        .collect();
    let deflated_sharpe_min = dsrs.iter().copied().fold(f64::INFINITY, f64::min);
    let deflated_sharpe_avg = if dsrs.is_empty() {
        f64::NAN
    } else {
        dsrs.iter().sum::<f64>() / dsrs.len() as f64
    };
    let pbos: Vec<f64> = folds.iter().filter_map(|f| f.pbo).collect();
    let pbo_avg = if pbos.is_empty() {
        None
    } else {
        Some(pbos.iter().sum::<f64>() / pbos.len() as f64)
    };
    let n_trials = folds.iter().map(|f| f.n_trials).max().unwrap_or(0);

    let holdout = if holdout_coins.is_empty() {
        None
    } else {
        let hc = holdout_coins.len().max(1);
        let total_pnl: f64 = holdout_coins.iter().map(|c| c.result.total_pnl).sum();
        let avg_sharpe: f64 = holdout_coins
            .iter()
            .map(|c| c.result.sharpe_ratio)
            .sum::<f64>()
            / hc as f64;
        let avg_max_dd: f64 = holdout_coins
            .iter()
            .map(|c| c.result.max_drawdown)
            .sum::<f64>()
            / hc as f64;
        let profitable_coins = holdout_coins
            .iter()
            .filter(|c| c.result.total_pnl > 0.0)
            .count();
        Some(HoldoutSummary {
            holdout_frac,
            n_coins: holdout_coins.len(),
            total_pnl,
            avg_sharpe,
            avg_max_dd,
            profitable_coins,
            coins: holdout_coins,
        })
    };

    let strategy = match alpha {
        AlphaKind::Funding => "funding_carry".to_string(),
        AlphaKind::Trend => "majors_alpha".to_string(),
    };
    let exec = match alpha {
        AlphaKind::Funding => funding_grid.exec.clone(),
        AlphaKind::Trend => trend_grid.exec.clone(),
    };

    let prereg_desc = crate::prereg::descriptor(series, alpha, funding_grid, trend_grid, opts);
    let prereg_hash = crate::prereg::hash(&prereg_desc);
    let generated_at = chrono::Utc::now().to_rfc3339();

    WalkForwardReport {
        schema_version: "1.2".into(),
        generated_at,
        alpha: strategy.clone(),
        strategy,
        initial_capital: opts.capital,
        n_coins: series.len(),
        n_folds: opts.n_folds,
        embargo_bars: opts.embargo_bars,
        holdout_frac,
        window: WindowMeta {
            n_folds: opts.n_folds,
            train_frac: opts.train_frac,
            expanding: opts.expanding,
            embargo_bars: opts.embargo_bars,
            holdout_frac,
        },
        exec,
        folds,
        test_total_pnl,
        test_avg_sharpe,
        test_avg_max_dd,
        test_win_rate,
        profitable_test_folds,
        deflated_sharpe_min: if deflated_sharpe_min.is_finite() {
            deflated_sharpe_min
        } else {
            f64::NAN
        },
        deflated_sharpe_avg,
        pbo_avg,
        n_trials,
        prereg_hash,
        prereg: Some(prereg_desc),
        holdout,
        realism_notes: vec![
            "taker_bps default 3.5 (HL taker)".into(),
            "base slip_bps + impact_bps * sqrt(participation)".into(),
            "next_bar + optional queue_delay_bars".into(),
            "holdout tail never used in grid-search".into(),
            "deflated_sharpe / pbo: selection bias corrected for n_trials grid trials (Bailey & Lopez de Prado)".into(),
            "sim vs paper protocol: same symbols, capital, risk_fraction, fees; target |ΔPnL| ≤ 5% on same window when both use these fills".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Bar;

    fn synth_bars(n: usize) -> Vec<Bar> {
        let mut bars = Vec::new();
        let mut mid = 100.0;
        for i in 0..n {
            mid *= if (i / 40) % 2 == 0 { 1.002 } else { 0.998 };
            bars.push(Bar::new(i as i64 * 3600, mid, 0.0));
        }
        bars
    }

    #[test]
    fn trend_wf_with_embargo() {
        let bars = synth_bars(200);
        let folds = walk_forward_coin(
            "TEST",
            &bars,
            10_000.0,
            3,
            0.7,
            AlphaKind::Trend,
            &FundingGrid::default(),
            &TrendGrid::default(),
            true,
            2,
        );
        assert!(!folds.is_empty());
    }

    #[test]
    fn holdout_never_in_train_grid() {
        let bars = synth_bars(300);
        let series = vec![("ETH".into(), bars)];
        let report = run_walk_forward_opts(
            &series,
            AlphaKind::Trend,
            &FundingGrid::default(),
            &TrendGrid::default(),
            &WfOptions {
                capital: 10_000.0,
                n_folds: 3,
                train_frac: 0.7,
                expanding: true,
                embargo_bars: 2,
                holdout_frac: 0.2,
            },
        );
        assert!(report.holdout.is_some());
        let h = report.holdout.unwrap();
        assert_eq!(h.n_coins, 1);
        assert!(h.coins[0].bars >= 24);
        assert_eq!(report.schema_version, "1.2");
        assert!(!report.generated_at.is_empty());
        assert!((report.exec.taker_bps - 3.5).abs() < 1e-9);
    }

    #[test]
    fn zero_holdout_skips_holdout_block() {
        let bars = synth_bars(200);
        let series = vec![("ETH".into(), bars)];
        let report = run_walk_forward(
            &series,
            10_000.0,
            3,
            0.7,
            AlphaKind::Funding,
            &FundingGrid::default(),
            &TrendGrid::default(),
            true,
            2,
        );
        assert!(report.holdout.is_none());
        assert_eq!(report.holdout_frac, 0.0);
    }
}
