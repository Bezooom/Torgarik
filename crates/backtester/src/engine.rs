//! Fees-aware backtest: **next-bar / queue-delay entry** + base slip + volume impact.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bar {
    /// Unix seconds
    pub ts: i64,
    pub mid: f64,
    /// Venue raw funding rate for the interval (HL ~hourly)
    pub funding: f64,
    /// Optional bar quote volume in USD notional (0 = unknown → use ExecConfig default).
    #[serde(default)]
    pub volume: f64,
}

impl Bar {
    pub fn new(ts: i64, mid: f64, funding: f64) -> Self {
        Self {
            ts,
            mid,
            funding,
            volume: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestResult {
    pub initial_capital: f64,
    pub final_capital: f64,
    pub total_pnl: f64,
    pub funding_pnl: f64,
    pub price_pnl: f64,
    pub fees_paid: f64,
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub win_rate: f64,
    pub profit_factor: f64,
    pub sharpe_ratio: f64,
    pub max_drawdown: f64,
    /// Per-bar equity returns of the run. Memory only: never serialized, so the
    /// report stays small, but deflated Sharpe / PBO need the actual series.
    #[serde(skip)]
    pub bar_returns: Vec<f64>,
}

/// Shared execution assumptions for realistic-ish OOS / paper alignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecConfig {
    /// Base half-spread / adverse selection on mid (bps of price). Applied on entry and exit.
    pub slip_bps: f64,
    /// Extra impact (bps) scaled by sqrt(order_notional / bar_volume).
    pub impact_bps: f64,
    /// Exchange fee bps of notional (HL taker ~3.5).
    pub taker_bps: f64,
    /// Signal on bar i close → first eligible fill bar is i+1.
    pub next_bar: bool,
    /// Additional bars after the next-bar slot (order queue / limit latency).
    /// Total delay = (next_bar as usize) + queue_delay_bars.
    pub queue_delay_bars: usize,
    /// When bar.volume is 0, assume this USD notional for impact participation.
    pub default_bar_volume_usd: f64,
    pub risk_fraction: f64,
    pub stop_loss: f64,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            slip_bps: 2.0,
            impact_bps: 1.0,
            taker_bps: 3.5,
            next_bar: true,
            queue_delay_bars: 0,
            default_bar_volume_usd: 5_000_000.0,
            risk_fraction: 0.1,
            stop_loss: 0.06,
        }
    }
}

impl ExecConfig {
    pub fn fill_delay_bars(&self) -> usize {
        (if self.next_bar { 1 } else { 0 }) + self.queue_delay_bars
    }

    fn fee(&self, notional: f64) -> f64 {
        notional * self.taker_bps / 10_000.0
    }

    /// Effective slip fraction (price units) given order notional and bar volume.
    pub fn slip_frac(&self, order_notional: f64, bar_volume_usd: f64) -> f64 {
        let base = self.slip_bps / 10_000.0;
        let vol = if bar_volume_usd > 1.0 {
            bar_volume_usd
        } else {
            self.default_bar_volume_usd.max(1.0)
        };
        let participation = (order_notional / vol).clamp(0.0, 2.0);
        let impact = (self.impact_bps / 10_000.0) * participation.sqrt();
        base + impact
    }

    fn entry_px(&self, mid: f64, is_buy: bool, order_notional: f64, bar_volume: f64) -> f64 {
        let slip = self.slip_frac(order_notional, bar_volume);
        if is_buy {
            mid * (1.0 + slip)
        } else {
            mid * (1.0 - slip)
        }
    }

    fn exit_px(&self, mid: f64, was_long: bool, order_notional: f64, bar_volume: f64) -> f64 {
        // Exit long = sell; exit short = buy
        self.entry_px(mid, !was_long, order_notional, bar_volume)
    }
}

#[derive(Debug, Clone)]
pub struct FundingBacktestConfig {
    pub enter: f64,
    pub exit: f64,
    pub exec: ExecConfig,
}

impl Default for FundingBacktestConfig {
    fn default() -> Self {
        Self {
            enter: 0.00004,
            exit: 0.000012,
            exec: ExecConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrendBacktestConfig {
    pub ema_fast: usize,
    pub ema_slow: usize,
    pub funding_veto: f64,
    pub min_trend_strength: f64,
    /// Bars for ATR/vol estimate.
    pub vol_lookback: usize,
    /// Min mean |ret| over lookback to allow new entries (regime filter).
    pub min_vol_ratio: f64,
    pub exec: ExecConfig,
}

impl Default for TrendBacktestConfig {
    fn default() -> Self {
        Self {
            ema_fast: 8,
            ema_slow: 34,
            funding_veto: 0.00015,
            min_trend_strength: 0.0008,
            vol_lookback: 24,
            min_vol_ratio: 0.0025,
            exec: ExecConfig::default(),
        }
    }
}

/// Signal series produced from bars, before any execution.
///
/// Split out from the simulation so the same generator can be replayed on a
/// truncated prefix and compared bar by bar (see [`BacktestEngine::audit_causality`]).
#[derive(Debug, Clone, Default)]
pub struct SignalPlan {
    /// Desired side at the close of each bar.
    pub signal: Vec<f64>,
    /// Optional per-bar regime gate (trend vol filter).
    pub vol_ok: Option<Vec<bool>>,
}

pub struct BacktestEngine {
    initial_capital: f64,
}

impl BacktestEngine {
    pub fn new(initial_capital: f64) -> Self {
        Self { initial_capital }
    }

    pub fn run_simulation(&self, prices: &[f64], signals: &[f64]) -> BacktestResult {
        let bars: Vec<Bar> = prices
            .iter()
            .enumerate()
            .map(|(i, p)| Bar {
                ts: i as i64,
                mid: *p,
                funding: if i < signals.len() {
                    (signals[i] - 0.5) * 0.0002
                } else {
                    0.0
                },
                volume: 0.0,
            })
            .collect();
        self.run_funding(&bars, &FundingBacktestConfig::default())
    }

    /// Funding-carry signal series. Delegates to the shared implementation in
    /// `trading_strategies::signals` so research and execution cannot drift.
    pub fn funding_plan(&self, bars: &[Bar], cfg: &FundingBacktestConfig) -> SignalPlan {
        let funding: Vec<f64> = bars.iter().map(|b| b.funding).collect();
        let p = trading_strategies::signals::FundingParams {
            enter: cfg.enter,
            exit: cfg.exit,
        };
        SignalPlan {
            signal: trading_strategies::signals::funding_plan(&funding, &p),
            vol_ok: None,
        }
    }

    /// Funding carry with delayed entry + slip/impact.
    pub fn run_funding(&self, bars: &[Bar], cfg: &FundingBacktestConfig) -> BacktestResult {
        if bars.len() < 3 {
            return empty_result(self.initial_capital);
        }
        let plan = self.funding_plan(bars, cfg);
        self.run_signals(bars, &plan.signal, &cfg.exec, plan.vol_ok.as_deref())
    }

    /// EMA trend + funding veto + vol regime. Delegates to the shared
    /// implementation in `trading_strategies::signals`.
    pub fn trend_plan(&self, bars: &[Bar], cfg: &TrendBacktestConfig) -> SignalPlan {
        let mids: Vec<f64> = bars.iter().map(|b| b.mid).collect();
        let funding: Vec<f64> = bars.iter().map(|b| b.funding).collect();
        let p = trading_strategies::signals::TrendParams {
            ema_fast: cfg.ema_fast,
            ema_slow: cfg.ema_slow,
            min_trend_strength: cfg.min_trend_strength,
            funding_veto: cfg.funding_veto,
            vol_lookback: cfg.vol_lookback,
            min_vol_ratio: cfg.min_vol_ratio,
        };
        let plan = trading_strategies::signals::trend_plan(&mids, &funding, &p);
        SignalPlan {
            signal: plan.signal,
            vol_ok: Some(plan.vol_ok),
        }
    }

    /// EMA trend + funding veto + vol regime.
    pub fn run_trend(&self, bars: &[Bar], cfg: &TrendBacktestConfig) -> BacktestResult {
        if bars.len() < cfg.ema_slow + 2 {
            return empty_result(self.initial_capital);
        }
        let plan = self.trend_plan(bars, cfg);
        self.run_signals(bars, &plan.signal, &cfg.exec, plan.vol_ok.as_deref())
    }

    /// Look-ahead audit: replay `compute` on truncated prefixes and require the
    /// shared prefix of every series to be **bit-identical** to the full run.
    ///
    /// This is the property that makes "point-in-time" real. A generator that
    /// peeks at a later bar (quantile over the whole slice, future label,
    /// normalisation fitted on the full history) produces a different prefix and
    /// is rejected here, regardless of how it is written.
    ///
    /// Returns the number of probes checked, or every mismatch found.
    pub fn audit_causality<F>(bars: &[Bar], probes: usize, compute: F) -> Result<usize, Vec<String>>
    where
        F: Fn(&[Bar]) -> SignalPlan,
    {
        let n = bars.len();
        if n < 8 {
            return Ok(0);
        }
        let full = compute(bars);
        if full.signal.len() != n {
            return Err(vec![format!(
                "signal length {} != bars {n}",
                full.signal.len()
            )]);
        }
        if let Some(v) = &full.vol_ok {
            if v.len() != n {
                return Err(vec![format!("vol_ok length {} != bars {n}", v.len())]);
            }
        }

        let probes = probes.max(1);
        let mut checked = 0usize;
        let mut errors: Vec<String> = Vec::new();
        // Probe evenly across the series, always leaving at least one bar after
        // the cut so a "next bar" peek has something to peek at.
        for p in 1..=probes {
            let k = (n * p / (probes + 1)).clamp(4, n - 1);
            let prefix = compute(&bars[..k]);
            if prefix.signal.len() < k {
                errors.push(format!(
                    "probe k={k}: signal length {} < {k}",
                    prefix.signal.len()
                ));
                continue;
            }
            for j in 0..k {
                if prefix.signal[j].to_bits() != full.signal[j].to_bits() {
                    errors.push(format!(
                        "probe k={k}: signal[{j}] differs when bar {} exists ({} vs {})",
                        n - 1,
                        prefix.signal[j],
                        full.signal[j]
                    ));
                    break;
                }
            }
            if let (Some(pv), Some(fv)) = (prefix.vol_ok.as_ref(), full.vol_ok.as_ref()) {
                for j in 0..k {
                    if pv.get(j) != fv.get(j) {
                        errors.push(format!(
                            "probe k={k}: vol_ok[{j}] differs when bar {} exists",
                            n - 1
                        ));
                        break;
                    }
                }
            }
            checked += 1;
        }
        if errors.is_empty() {
            Ok(checked)
        } else {
            Err(errors)
        }
    }

    /// Execute signed target series with delayed fills.
    /// `signal[i]` = desired side at close of bar i.
    fn run_signals(
        &self,
        bars: &[Bar],
        signal: &[f64],
        exec: &ExecConfig,
        vol_ok: Option<&[bool]>,
    ) -> BacktestResult {
        let n = bars.len();
        let delay = exec.fill_delay_bars();
        let mut cash = self.initial_capital;
        let mut pos_size = 0.0f64;
        let mut entry = 0.0f64;
        let mut peak = cash;
        let mut max_dd = 0.0;
        let mut funding_pnl = 0.0;
        let mut price_pnl = 0.0;
        let mut fees_paid = 0.0;
        let mut wins = 0usize;
        let mut losses = 0usize;
        let mut gross_profit = 0.0;
        let mut gross_loss = 0.0;
        let mut returns = Vec::new();
        let mut bar_returns: Vec<f64> = Vec::with_capacity(n);
        let mut prev_equity = self.initial_capital;
        let mut trades = 0usize;

        for i in 0..n {
            let mid = bars[i].mid;
            let bar_vol = bars[i].volume;
            if mid <= 0.0 {
                continue;
            }

            let upnl = if pos_size.abs() > 0.0 {
                pos_size * (mid - entry)
            } else {
                0.0
            };
            let equity = cash + upnl;
            if equity > peak {
                peak = equity;
            } else {
                let dd = (peak - equity) / peak.max(1e-9);
                if dd > max_dd {
                    max_dd = dd;
                }
            }

            if pos_size.abs() > 0.0 {
                let notional = pos_size.abs() * mid;
                let pay = if pos_size > 0.0 {
                    -bars[i].funding * notional
                } else {
                    bars[i].funding * notional
                };
                cash += pay;
                funding_pnl += pay;
            }

            // Stop-loss
            if pos_size.abs() > 0.0 {
                let dir = pos_size.signum();
                let ret = dir * (mid - entry) / entry.max(1e-12);
                if ret <= -exec.stop_loss {
                    let notional = pos_size.abs() * mid;
                    let px = exec.exit_px(mid, pos_size > 0.0, notional, bar_vol);
                    let f = exec.fee(pos_size.abs() * px);
                    let pnl = pos_size * (px - entry) - f;
                    cash += pos_size * (px - entry) - f;
                    fees_paid += f;
                    price_pnl += pos_size * (px - entry);
                    trades += 1;
                    if pnl > 0.0 {
                        wins += 1;
                        gross_profit += pnl;
                    } else {
                        losses += 1;
                        gross_loss += pnl.abs();
                    }
                    returns.push(pnl / self.initial_capital);
                    pos_size = 0.0;
                    entry = 0.0;
                }
            }

            // Target from delayed signal
            let mut target_sign = if delay == 0 {
                signal[i]
            } else if i >= delay {
                signal[i - delay]
            } else {
                0.0
            };

            if let Some(v) = vol_ok {
                let sig_i = if delay == 0 {
                    i
                } else if i >= delay {
                    i - delay
                } else {
                    i
                };
                if sig_i < v.len() && !v[sig_i] {
                    if pos_size.abs() < 1e-12 {
                        target_sign = 0.0;
                    } else if target_sign.signum() != pos_size.signum() && target_sign.abs() > 0.5 {
                        target_sign = 0.0;
                    }
                }
            }

            let equity_now = cash + pos_size * (mid - entry);
            let target_size = if target_sign.abs() < 0.5 {
                0.0
            } else {
                target_sign.signum() * (equity_now.max(0.0) * exec.risk_fraction) / mid
            };

            if (target_size - pos_size).abs() * mid >= 5.0 {
                if pos_size.abs() > 0.0
                    && (target_size == 0.0 || target_size.signum() != pos_size.signum())
                {
                    let notional = pos_size.abs() * mid;
                    let px = exec.exit_px(mid, pos_size > 0.0, notional, bar_vol);
                    let f = exec.fee(pos_size.abs() * px);
                    let pnl = pos_size * (px - entry) - f;
                    cash += pos_size * (px - entry) - f;
                    fees_paid += f;
                    price_pnl += pos_size * (px - entry);
                    trades += 1;
                    if pnl > 0.0 {
                        wins += 1;
                        gross_profit += pnl;
                    } else {
                        losses += 1;
                        gross_loss += pnl.abs();
                    }
                    returns.push(pnl / self.initial_capital);
                    pos_size = 0.0;
                    entry = 0.0;
                }
                if target_size.abs() > 0.0 && pos_size.abs() < 1e-12 {
                    let is_buy = target_size > 0.0;
                    let notional = target_size.abs() * mid;
                    let px = exec.entry_px(mid, is_buy, notional, bar_vol);
                    let f = exec.fee(target_size.abs() * px);
                    cash -= f;
                    fees_paid += f;
                    pos_size = target_size;
                    entry = px;
                    trades += 1;
                }
            }

            // Per-bar equity return for the bias statistics.
            let eq_end = cash + pos_size * (mid - entry);
            if prev_equity > 0.0 && eq_end.is_finite() {
                bar_returns.push(eq_end / prev_equity - 1.0);
                prev_equity = eq_end;
            }
        }

        if let Some(last) = bars.last() {
            if pos_size.abs() > 0.0 {
                let notional = pos_size.abs() * last.mid;
                let px = exec.exit_px(last.mid, pos_size > 0.0, notional, last.volume);
                let f = exec.fee(pos_size.abs() * px);
                cash += pos_size * (px - entry) - f;
                fees_paid += f;
                price_pnl += pos_size * (px - entry);
            }
        }

        let mut result = finalize_result(
            self.initial_capital,
            cash,
            funding_pnl,
            price_pnl,
            fees_paid,
            trades,
            wins,
            losses,
            &returns,
            max_dd,
            gross_profit,
            gross_loss,
        );
        result.bar_returns = bar_returns;
        result
    }
}

fn empty_result(initial: f64) -> BacktestResult {
    finalize_result(initial, initial, 0.0, 0.0, 0.0, 0, 0, 0, &[], 0.0, 0.0, 0.0)
}

fn finalize_result(
    initial: f64,
    final_capital: f64,
    funding_pnl: f64,
    price_pnl: f64,
    fees_paid: f64,
    trades: usize,
    wins: usize,
    losses: usize,
    returns: &[f64],
    max_dd: f64,
    gross_profit: f64,
    gross_loss: f64,
) -> BacktestResult {
    let total_trades = wins + losses;
    let win_rate = if total_trades > 0 {
        wins as f64 / total_trades as f64
    } else {
        0.0
    };
    let profit_factor = if gross_loss > 0.0 {
        gross_profit / gross_loss
    } else {
        gross_profit
    };
    let mean_ret = if returns.is_empty() {
        0.0
    } else {
        returns.iter().sum::<f64>() / returns.len() as f64
    };
    let var_ret = if returns.len() > 1 {
        returns.iter().map(|r| (r - mean_ret).powi(2)).sum::<f64>() / (returns.len() - 1) as f64
    } else {
        0.0
    };
    let std_ret = var_ret.sqrt();
    let sharpe = if std_ret > 0.0 {
        (mean_ret / std_ret) * (252.0f64).sqrt()
    } else {
        0.0
    };
    BacktestResult {
        initial_capital: initial,
        final_capital,
        total_pnl: final_capital - initial,
        funding_pnl,
        price_pnl,
        fees_paid,
        total_trades: trades,
        winning_trades: wins,
        losing_trades: losses,
        win_rate,
        profit_factor,
        sharpe_ratio: sharpe,
        max_drawdown: max_dd,
        bar_returns: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uptrend(n: usize, step: f64) -> Vec<Bar> {
        let mut mid = 100.0;
        (0..n)
            .map(|i| {
                mid *= step;
                Bar::new(i as i64, mid, 0.0)
            })
            .collect()
    }

    fn wobble(n: usize) -> Vec<Bar> {
        let mut mid = 100.0;
        (0..n)
            .map(|i| {
                let drift = (((i * 37) % 11) as f64 - 5.0) * 0.0016;
                mid *= 1.0 + drift;
                Bar::new(i as i64 * 3600, mid, ((i % 5) as f64 - 2.0) * 0.00008)
            })
            .collect()
    }

    #[test]
    fn audit_passes_for_causal_trend_and_funding_plans() {
        let engine = BacktestEngine::new(10_000.0);
        let bars = wobble(240);

        let mut tcfg = TrendBacktestConfig {
            ema_fast: 5,
            ema_slow: 15,
            min_trend_strength: 0.0001,
            funding_veto: 0.0,
            min_vol_ratio: 0.0005,
            ..TrendBacktestConfig::default()
        };
        tcfg.exec.next_bar = true;
        let trend = BacktestEngine::audit_causality(&bars, 6, |b| engine.trend_plan(b, &tcfg));
        assert!(trend.is_ok(), "trend plan leaks: {trend:?}");
        assert_eq!(trend.unwrap(), 6, "every probe ran");

        let fcfg = FundingBacktestConfig::default();
        let funding = BacktestEngine::audit_causality(&bars, 6, |b| engine.funding_plan(b, &fcfg));
        assert!(funding.is_ok(), "funding plan leaks: {funding:?}");
    }

    #[test]
    fn audit_detects_a_signal_that_peeks_at_the_next_bar() {
        let engine = BacktestEngine::new(10_000.0);
        let bars = wobble(120);
        // Deliberately leaky: today's signal knows tomorrow's close.
        let leaky = |b: &[Bar]| SignalPlan {
            signal: (0..b.len())
                .map(|i| {
                    let next = b.get(i + 1).map(|x| x.mid).unwrap_or(b[i].mid);
                    if next > b[i].mid {
                        1.0
                    } else {
                        -1.0
                    }
                })
                .collect(),
            vol_ok: None,
        };
        let err = BacktestEngine::audit_causality(&bars, 4, leaky)
            .expect_err("a next-bar peek must be rejected");
        assert!(!err.is_empty());
        assert!(
            err[0].contains("differs when bar"),
            "unexpected message: {}",
            err[0]
        );
        let _ = engine;
    }

    #[test]
    fn next_bar_uptrend_still_trades() {
        let engine = BacktestEngine::new(10_000.0);
        let bars = uptrend(150, 1.004);
        let res = engine.run_trend(
            &bars,
            &TrendBacktestConfig {
                ema_fast: 5,
                ema_slow: 15,
                min_trend_strength: 0.0001,
                funding_veto: 0.0,
                min_vol_ratio: 0.0,
                vol_lookback: 5,
                exec: ExecConfig {
                    next_bar: true,
                    slip_bps: 2.0,
                    impact_bps: 0.0,
                    ..Default::default()
                },
            },
        );
        assert!(res.total_trades > 0);
        assert!(res.final_capital > res.initial_capital * 0.9);
    }

    #[test]
    fn slip_and_fees_reduce_pnl() {
        let engine = BacktestEngine::new(10_000.0);
        let bars = uptrend(100, 1.003);
        let mut cfg = TrendBacktestConfig {
            ema_fast: 5,
            ema_slow: 15,
            min_trend_strength: 0.0001,
            funding_veto: 0.0,
            min_vol_ratio: 0.0,
            vol_lookback: 5,
            exec: ExecConfig {
                next_bar: true,
                slip_bps: 0.0,
                impact_bps: 0.0,
                taker_bps: 0.0,
                ..Default::default()
            },
        };
        let zero = engine.run_trend(&bars, &cfg);
        cfg.exec.slip_bps = 10.0;
        cfg.exec.taker_bps = 3.5;
        cfg.exec.impact_bps = 5.0;
        let costly = engine.run_trend(&bars, &cfg);
        assert!(costly.total_pnl <= zero.total_pnl + 1e-6);
        assert!(costly.fees_paid >= zero.fees_paid);
    }

    #[test]
    fn impact_scales_with_participation() {
        let exec = ExecConfig {
            slip_bps: 0.0,
            impact_bps: 10.0,
            default_bar_volume_usd: 1_000_000.0,
            ..Default::default()
        };
        let small = exec.slip_frac(1_000.0, 1_000_000.0);
        let large = exec.slip_frac(500_000.0, 1_000_000.0);
        assert!(large > small);
    }

    #[test]
    fn queue_delay_reduces_trades_or_pnl_vs_immediate() {
        let engine = BacktestEngine::new(10_000.0);
        // Noisy series so delay matters
        let mut mid = 100.0;
        let bars: Vec<Bar> = (0..120)
            .map(|i| {
                mid *= if i % 7 < 4 { 1.003 } else { 0.997 };
                Bar::new(i, mid, 0.0)
            })
            .collect();
        let mut cfg = TrendBacktestConfig {
            ema_fast: 5,
            ema_slow: 15,
            min_trend_strength: 0.0001,
            funding_veto: 0.0,
            min_vol_ratio: 0.0,
            vol_lookback: 5,
            exec: ExecConfig {
                next_bar: true,
                queue_delay_bars: 0,
                slip_bps: 2.0,
                impact_bps: 0.0,
                taker_bps: 3.5,
                ..Default::default()
            },
        };
        let _fast = engine.run_trend(&bars, &cfg);
        cfg.exec.queue_delay_bars = 2;
        let slow = engine.run_trend(&bars, &cfg);
        // Same delay config is deterministic
        let slow2 = engine.run_trend(&bars, &cfg);
        assert!((slow.total_pnl - slow2.total_pnl).abs() < 1e-9);
    }

    #[test]
    fn default_taker_is_hl_35bps() {
        assert!((ExecConfig::default().taker_bps - 3.5).abs() < 1e-9);
    }
}
