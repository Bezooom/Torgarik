//! Russian personal income tax (НДФЛ) and FIFO lot accounting.
//!
//! Crypto (Hyperliquid) and MOEX positions are tracked as lots priced in roubles:
//! the caller converts USD/USDC amounts to RUB at the ЦБ rate of the acquisition
//! date, and proceeds the same way. Every lot and every realized trade in this
//! module is therefore already denominated in RUB.
//!
//! The model is deliberately simplified:
//! - FIFO cost basis per instrument (НК РФ ст. 214.1 for securities; crypto uses
//!   the same FIFO convention here).
//! - Two-tier progressive rate: 13% on the part of the base up to 2.4M RUB and
//!   15% above it.
//! - Dividends are folded into the same base and taxed at the same two-tier
//!   rate. In practice a Russian broker or the issuer withholds dividend tax at
//!   source, so a withheld amount must be excluded from [`AnnualTaxReport::add_dividend`]
//!   (or passed to [`AnnualTaxReport::add_fee`]) to avoid double taxation.

use std::fmt;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// Upper bound of the lower НДФЛ bracket, in roubles.
pub const THRESHOLD_RUB: f64 = 2_400_000.0;
/// НДФЛ rate applied to the part of the base up to [`THRESHOLD_RUB`].
pub const RATE_LOW: f64 = 0.13;
/// НДФЛ rate applied to the part of the base above [`THRESHOLD_RUB`].
pub const RATE_HIGH: f64 = 0.15;
/// Minimum holding period, in full years, for the ИИС-3 exemption.
pub const IIS3_MIN_YEARS: i64 = 5;

/// Tolerance for floating-point quantity comparisons.
const QTY_EPS: f64 = 1e-9;

/// Currency of a quoted amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Currency {
    /// Russian rouble, the tax reporting currency.
    Rub,
    /// US dollar, converted to RUB at the ЦБ rate.
    Usd,
    /// USD Coin, converted to RUB at the ЦБ rate.
    Usdc,
}

impl Currency {
    /// Short currency code as used by venues and reports.
    pub fn code(self) -> &'static str {
        match self {
            Currency::Rub => "RUB",
            Currency::Usd => "USD",
            Currency::Usdc => "USDC",
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Asset class, which decides the reporting bucket an instrument belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetClass {
    /// Crypto asset (Hyperliquid perpetuals, spot coins).
    Crypto,
    /// Listed equity (MOEX shares, foreign stock).
    Equity,
    /// Debt instrument (OFZ, corporate bonds).
    Bond,
    /// Collective investment vehicle (mutual fund, ETF).
    Fund,
    /// Bank deposit.
    Deposit,
}

impl AssetClass {
    /// Stable lower-case identifier, suitable for logs and report columns.
    pub fn code(self) -> &'static str {
        match self {
            AssetClass::Crypto => "crypto",
            AssetClass::Equity => "equity",
            AssetClass::Bond => "bond",
            AssetClass::Fund => "fund",
            AssetClass::Deposit => "deposit",
        }
    }
}

impl fmt::Display for AssetClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// A single acquisition lot with its cost basis already expressed in roubles.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TaxLot {
    /// Remaining quantity in the lot (units of the instrument).
    pub qty: f64,
    /// Cost of one unit in RUB at the acquisition date.
    pub unit_cost_rub: f64,
    /// Acquisition timestamp (UTC).
    pub acquired: DateTime<Utc>,
}

impl TaxLot {
    /// Total cost basis of the remaining quantity, in RUB.
    pub fn cost_basis_rub(&self) -> f64 {
        self.qty * self.unit_cost_rub
    }
}

/// Result of consuming lots in a FIFO sale.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RealizedTrade {
    /// Sold quantity.
    pub qty: f64,
    /// Sale proceeds in RUB (`qty * unit price`).
    pub proceeds_rub: f64,
    /// FIFO cost basis of the consumed quantity, in RUB.
    pub cost_rub: f64,
    /// Realized profit or loss in RUB (`proceeds_rub - cost_rub`, may be negative).
    pub pnl_rub: f64,
    /// Holding period in whole days, weighted by quantity across consumed lots
    /// and rounded to the nearest day. Exact for a single-lot sale.
    pub holding_days: i64,
    /// Fractional quantity-weighted holding period in days. Negative only if the
    /// sale timestamp precedes the acquisition timestamp (bad input data).
    pub avg_holding_days: f64,
}

/// FIFO lot ledger for a single instrument.
///
/// Lots are kept in acquisition order. A sale always consumes the oldest lot
/// first, may span several lots, and may consume a lot partially.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LotBook {
    lots: Vec<TaxLot>,
}

impl LotBook {
    /// Create an empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an acquisition lot.
    ///
    /// Fails when `qty` or `unit_cost_rub` is non-finite or not strictly positive.
    pub fn buy(&mut self, qty: f64, unit_cost_rub: f64, ts: DateTime<Utc>) -> Result<(), String> {
        if !qty.is_finite() || qty <= 0.0 {
            return Err(format!(
                "lot quantity must be finite and positive, got {qty}"
            ));
        }
        if !unit_cost_rub.is_finite() || unit_cost_rub <= 0.0 {
            return Err(format!(
                "lot unit cost must be finite and positive, got {unit_cost_rub}"
            ));
        }
        self.lots.push(TaxLot {
            qty,
            unit_cost_rub,
            acquired: ts,
        });
        Ok(())
    }

    /// Sell `qty` units at `unit_price_rub`, consuming the oldest lots first.
    ///
    /// A zero price is accepted (forfeiture, transfer at no consideration); a
    /// negative or non-finite price is rejected. Fails when `qty` is
    /// non-finite/not positive or exceeds the quantity currently held, in which
    /// case the ledger is left untouched.
    pub fn sell(
        &mut self,
        qty: f64,
        unit_price_rub: f64,
        ts: DateTime<Utc>,
    ) -> Result<RealizedTrade, String> {
        if !qty.is_finite() || qty <= 0.0 {
            return Err(format!(
                "sell quantity must be finite and positive, got {qty}"
            ));
        }
        if !unit_price_rub.is_finite() || unit_price_rub < 0.0 {
            return Err(format!(
                "sell unit price must be finite and non-negative, got {unit_price_rub}"
            ));
        }
        let held = self.held_qty();
        if qty > held + QTY_EPS {
            return Err(format!("cannot sell {qty}: only {held} held"));
        }

        // Plan the consumption first so that an unsatisfiable request leaves the
        // ledger unchanged.
        let mut remaining = qty;
        let mut cost_rub = 0.0;
        let mut weighted_days = 0.0;
        let mut consumed = 0usize;
        let mut last_take = 0.0;

        for lot in &self.lots {
            if remaining <= QTY_EPS {
                break;
            }
            let take = remaining.min(lot.qty);
            cost_rub += take * lot.unit_cost_rub;
            weighted_days += take * whole_days_between(lot.acquired, ts) as f64;
            remaining -= take;
            last_take = take;
            consumed += 1;
        }
        if remaining > QTY_EPS {
            return Err(format!("cannot sell {qty}: only {} held", held - remaining));
        }

        // Apply: drop fully consumed lots, shrink the partially consumed one.
        self.lots.drain(..consumed.saturating_sub(1));
        if let Some(front) = self.lots.first_mut() {
            front.qty -= last_take;
            if front.qty <= QTY_EPS {
                self.lots.remove(0);
            }
        }

        let proceeds_rub = qty * unit_price_rub;
        let avg_holding_days = weighted_days / qty;
        Ok(RealizedTrade {
            qty,
            proceeds_rub,
            cost_rub,
            pnl_rub: proceeds_rub - cost_rub,
            holding_days: avg_holding_days.round() as i64,
            avg_holding_days,
        })
    }

    /// Total quantity currently held.
    pub fn held_qty(&self) -> f64 {
        self.lots.iter().map(|lot| lot.qty).sum()
    }

    /// Remaining lots in FIFO order (oldest first).
    pub fn lots(&self) -> &[TaxLot] {
        &self.lots
    }
}

/// Taxable events accumulated for one calendar year.
///
/// Dividends normally have tax withheld at source by the broker or issuer; this
/// report folds them into the same base only when the caller has not done so.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnualTaxReport {
    year: i32,
    realized_pnl_rub: f64,
    dividends_rub: f64,
    fees_rub: f64,
}

impl AnnualTaxReport {
    /// Create a report for the given calendar year.
    pub fn new(year: i32) -> Self {
        Self {
            year,
            realized_pnl_rub: 0.0,
            dividends_rub: 0.0,
            fees_rub: 0.0,
        }
    }

    /// Reporting year.
    pub fn year(&self) -> i32 {
        self.year
    }

    /// Add a realized trade result (profit or loss).
    pub fn add_realized(&mut self, trade: &RealizedTrade) {
        self.realized_pnl_rub += trade.pnl_rub;
    }

    /// Add a dividend received, in RUB.
    ///
    /// Fails when the amount is non-finite or negative.
    pub fn add_dividend(&mut self, amount_rub: f64) -> Result<(), String> {
        if !amount_rub.is_finite() || amount_rub < 0.0 {
            return Err(format!(
                "dividend must be finite and non-negative, got {amount_rub}"
            ));
        }
        self.dividends_rub += amount_rub;
        Ok(())
    }

    /// Add fees (broker commission, exchange fee), which reduce the base.
    ///
    /// Fails when the amount is non-finite or negative.
    pub fn add_fee(&mut self, fee_rub: f64) -> Result<(), String> {
        if !fee_rub.is_finite() || fee_rub < 0.0 {
            return Err(format!(
                "fee must be finite and non-negative, got {fee_rub}"
            ));
        }
        self.fees_rub += fee_rub;
        Ok(())
    }

    /// Accumulated realized profit or loss, in RUB.
    pub fn realized_pnl_rub(&self) -> f64 {
        self.realized_pnl_rub
    }

    /// Accumulated dividends, in RUB.
    pub fn dividends_rub(&self) -> f64 {
        self.dividends_rub
    }

    /// Accumulated fees, in RUB.
    pub fn fees_rub(&self) -> f64 {
        self.fees_rub
    }

    /// Taxable base: `max(0, realized + dividends - fees)`.
    ///
    /// A loss year yields a zero base; a loss is never carried forward in this
    /// simplified model.
    pub fn tax_base(&self) -> f64 {
        (self.realized_pnl_rub + self.dividends_rub - self.fees_rub).max(0.0)
    }

    /// НДФЛ due on the base, rounded to whole roubles (НК РФ ст. 52).
    ///
    /// 13% up to [`THRESHOLD_RUB`], 15% on the excess above it.
    pub fn tax_due(&self) -> f64 {
        two_tier_ndfl(self.tax_base()).round()
    }

    /// Taxable base minus the tax due on it.
    pub fn net_after_tax(&self) -> f64 {
        self.tax_base() - self.tax_due()
    }
}

/// Taxable account type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaxAccount {
    /// Ordinary brokerage account: trade results are taxed in full.
    Broker,
    /// ИИС-1: gives a contribution deduction, not an exemption from the
    /// trade-result tax, so it is taxed like a brokerage account here.
    Iis1,
    /// ИИС-3: the whole account is exempt once held for [`IIS3_MIN_YEARS`].
    Iis3 {
        /// Account opening date.
        opened: NaiveDate,
    },
}

impl TaxAccount {
    /// Account type label.
    pub fn kind(&self) -> &'static str {
        match self {
            TaxAccount::Broker => "broker",
            TaxAccount::Iis1 => "iis1",
            TaxAccount::Iis3 { .. } => "iis3",
        }
    }

    /// Whether the account is exempt from the trade-result tax on `today`.
    ///
    /// Only ИИС-3 is exempt, and only after [`IIS3_MIN_YEARS`] full calendar
    /// years have elapsed since `opened`. ИИС-1 is not exempt (it grants a
    /// contribution deduction instead), and neither is a brokerage account.
    pub fn is_exempt(&self, today: NaiveDate) -> bool {
        match self {
            TaxAccount::Iis3 { opened } => full_years_between(*opened, today) >= IIS3_MIN_YEARS,
            TaxAccount::Broker | TaxAccount::Iis1 => false,
        }
    }

    /// Tax due for `report` on this account as of `today`.
    ///
    /// Returns 0.0 for an exempt ИИС-3; otherwise the report's own tax.
    pub fn tax_due_for(&self, report: &AnnualTaxReport, today: NaiveDate) -> f64 {
        if self.is_exempt(today) {
            0.0
        } else {
            report.tax_due()
        }
    }
}

/// Progressive НДФЛ on a non-negative base, before rounding.
fn two_tier_ndfl(base: f64) -> f64 {
    if base <= 0.0 {
        0.0
    } else if base <= THRESHOLD_RUB {
        base * RATE_LOW
    } else {
        THRESHOLD_RUB * RATE_LOW + (base - THRESHOLD_RUB) * RATE_HIGH
    }
}

/// Whole days from `from` to `to`; truncates toward zero.
fn whole_days_between(from: DateTime<Utc>, to: DateTime<Utc>) -> i64 {
    to.signed_duration_since(from).num_days()
}

/// Number of complete calendar years between `from` and `to` (0 when inverted).
fn full_years_between(from: NaiveDate, to: NaiveDate) -> i64 {
    let mut years = to.year() as i64 - from.year() as i64;
    if (to.month(), to.day()) < (from.month(), from.day()) {
        years -= 1;
    }
    years.max(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 12, 0, 0)
            .single()
            .expect("valid timestamp")
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    fn close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected {expected}, got {actual}"
        );
    }

    fn trade(pnl_rub: f64) -> RealizedTrade {
        RealizedTrade {
            qty: 1.0,
            proceeds_rub: pnl_rub.max(0.0),
            cost_rub: (-pnl_rub).max(0.0),
            pnl_rub,
            holding_days: 0,
            avg_holding_days: 0.0,
        }
    }

    #[test]
    fn fifo_consumes_oldest_lot_first() {
        let mut book = LotBook::new();
        book.buy(10.0, 100.0, ts(2024, 1, 1)).expect("buy lot 1");
        book.buy(10.0, 200.0, ts(2024, 2, 1)).expect("buy lot 2");

        let sold = book.sell(10.0, 300.0, ts(2024, 3, 1)).expect("sell");
        close(sold.cost_rub, 1_000.0);
        close(sold.proceeds_rub, 3_000.0);
        close(sold.pnl_rub, 2_000.0);
        assert_eq!(sold.holding_days, 60);

        close(book.held_qty(), 10.0);
        assert_eq!(book.lots().len(), 1);
        close(book.lots()[0].unit_cost_rub, 200.0);
    }

    #[test]
    fn sell_spans_two_lots() {
        let mut book = LotBook::new();
        book.buy(5.0, 100.0, ts(2024, 1, 1)).expect("buy lot 1");
        book.buy(5.0, 200.0, ts(2024, 1, 2)).expect("buy lot 2");

        let sold = book.sell(6.0, 300.0, ts(2024, 1, 11)).expect("sell");
        close(sold.cost_rub, 500.0 + 200.0);
        close(sold.proceeds_rub, 1_800.0);
        close(sold.pnl_rub, 1_100.0);
        close(book.held_qty(), 4.0);
        assert_eq!(book.lots().len(), 1);
        close(book.lots()[0].qty, 4.0);
        close(book.lots()[0].unit_cost_rub, 200.0);
    }

    #[test]
    fn partial_lot_is_left_at_the_front() {
        let mut book = LotBook::new();
        book.buy(10.0, 100.0, ts(2024, 5, 1)).expect("buy");

        let sold = book.sell(3.0, 150.0, ts(2024, 6, 1)).expect("sell");
        close(sold.cost_rub, 300.0);
        close(sold.proceeds_rub, 450.0);
        close(sold.pnl_rub, 150.0);
        close(book.held_qty(), 7.0);
        close(book.lots()[0].qty, 7.0);
        close(book.lots()[0].cost_basis_rub(), 700.0);
    }

    #[test]
    fn selling_more_than_held_errors_and_keeps_ledger() {
        let mut book = LotBook::new();
        book.buy(1.0, 100.0, ts(2024, 1, 1)).expect("buy");

        let err = book.sell(2.0, 120.0, ts(2024, 1, 2)).err().expect("error");
        assert!(err.contains("only"), "unexpected message: {err}");
        close(book.held_qty(), 1.0);
        assert_eq!(book.lots().len(), 1);
    }

    #[test]
    fn selling_exactly_held_empties_the_book() {
        let mut book = LotBook::new();
        book.buy(2.0, 50.0, ts(2024, 1, 1)).expect("buy");
        let sold = book.sell(2.0, 60.0, ts(2024, 1, 3)).expect("sell");
        close(sold.pnl_rub, 20.0);
        close(book.held_qty(), 0.0);
        assert!(book.lots().is_empty());
    }

    #[test]
    fn invalid_buy_is_rejected() {
        let mut book = LotBook::new();
        assert!(book.buy(0.0, 100.0, ts(2024, 1, 1)).is_err());
        assert!(book.buy(-1.0, 100.0, ts(2024, 1, 1)).is_err());
        assert!(book.buy(f64::NAN, 100.0, ts(2024, 1, 1)).is_err());
        assert!(book.buy(f64::INFINITY, 100.0, ts(2024, 1, 1)).is_err());
        assert!(book.buy(1.0, 0.0, ts(2024, 1, 1)).is_err());
        assert!(book.buy(1.0, f64::NAN, ts(2024, 1, 1)).is_err());
        assert!(book.lots().is_empty());
    }

    #[test]
    fn holding_days_are_weighted_by_quantity() {
        let mut book = LotBook::new();
        book.buy(10.0, 100.0, ts(2024, 1, 1)).expect("buy lot 1");
        book.buy(10.0, 100.0, ts(2024, 1, 21)).expect("buy lot 2");

        let sold = book.sell(20.0, 100.0, ts(2024, 1, 31)).expect("sell");
        // 30 days on the first lot, 10 days on the second.
        close(sold.avg_holding_days, 20.0);
        assert_eq!(sold.holding_days, 20);
    }

    #[test]
    fn fractional_holding_days_round_to_whole_days() {
        let mut book = LotBook::new();
        book.buy(1.0, 100.0, ts(2024, 1, 1)).expect("buy lot 1");
        book.buy(1.0, 100.0, ts(2024, 1, 2)).expect("buy lot 2");

        let sold = book.sell(2.0, 100.0, ts(2024, 1, 3)).expect("sell");
        close(sold.avg_holding_days, 1.5);
        assert_eq!(sold.holding_days, 2);
    }

    #[test]
    fn tax_at_exact_threshold() {
        let mut report = AnnualTaxReport::new(2025);
        report.add_realized(&trade(THRESHOLD_RUB));
        close(report.tax_base(), THRESHOLD_RUB);
        // 2_400_000 * 0.13
        close(report.tax_due(), 312_000.0);
    }

    #[test]
    fn tax_above_threshold_uses_two_tiers() {
        let mut report = AnnualTaxReport::new(2025);
        report.add_realized(&trade(3_400_000.0));
        // 312_000 + 1_000_000 * 0.15
        close(report.tax_due(), 462_000.0);
        close(report.net_after_tax(), 3_400_000.0 - 462_000.0);
    }

    #[test]
    fn fees_reduce_the_base() {
        let mut report = AnnualTaxReport::new(2025);
        report.add_realized(&trade(100_000.0));
        report.add_fee(20_000.0).expect("fee");
        close(report.tax_base(), 80_000.0);
        close(report.tax_due(), 10_400.0);
    }

    #[test]
    fn dividends_add_to_the_base() {
        let mut report = AnnualTaxReport::new(2025);
        report.add_realized(&trade(100_000.0));
        report.add_dividend(100_000.0).expect("dividend");
        close(report.tax_base(), 200_000.0);
        close(report.tax_due(), 26_000.0);
    }

    #[test]
    fn loss_year_never_produces_negative_tax() {
        let mut report = AnnualTaxReport::new(2025);
        report.add_realized(&trade(-50_000.0));
        report.add_dividend(10_000.0).expect("dividend");
        report.add_fee(5_000.0).expect("fee");
        close(report.tax_base(), 0.0);
        close(report.tax_due(), 0.0);
        close(report.net_after_tax(), 0.0);
    }

    #[test]
    fn tax_due_is_rounded_to_whole_roubles() {
        let mut report = AnnualTaxReport::new(2025);
        report.add_realized(&trade(10.0));
        // 10 * 0.13 = 1.3 -> 1 rouble.
        close(report.tax_due(), 1.0);
        close(report.net_after_tax(), 9.0);
    }

    #[test]
    fn iis3_is_exempt_after_five_full_years() {
        let account = TaxAccount::Iis3 {
            opened: date(2020, 3, 1),
        };
        assert!(!account.is_exempt(date(2025, 2, 28)));
        assert!(account.is_exempt(date(2025, 3, 1)));
        assert!(account.is_exempt(date(2026, 1, 1)));
    }

    #[test]
    fn iis3_is_not_exempt_before_five_years() {
        let account = TaxAccount::Iis3 {
            opened: date(2021, 6, 15),
        };
        let mut report = AnnualTaxReport::new(2025);
        report.add_realized(&trade(1_000_000.0));

        assert!(!account.is_exempt(date(2025, 6, 14)));
        close(account.tax_due_for(&report, date(2025, 6, 14)), 130_000.0);
        close(account.tax_due_for(&report, date(2026, 6, 15)), 0.0);
    }

    #[test]
    fn iis1_is_not_exempt() {
        let account = TaxAccount::Iis1;
        let mut report = AnnualTaxReport::new(2025);
        report.add_realized(&trade(1_000_000.0));

        assert!(!account.is_exempt(date(2030, 1, 1)));
        close(account.tax_due_for(&report, date(2030, 1, 1)), 130_000.0);
    }

    #[test]
    fn broker_account_pays_tax() {
        let account = TaxAccount::Broker;
        let mut report = AnnualTaxReport::new(2025);
        report.add_realized(&trade(3_400_000.0));

        assert!(!account.is_exempt(date(2030, 1, 1)));
        close(account.tax_due_for(&report, date(2030, 1, 1)), 462_000.0);
    }
}
