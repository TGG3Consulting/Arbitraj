//! Файл: core/settlement_entry.rs.
//! Что делает: сравнивает размеры при реальной конверсии и передаёт лучший чистый результат общему исполнителю.
//! Кто вызывает: штатный расчёт кандидата и предпроверка.
//! От чего зависит: та же сетка поиска, резерв, правила пары и пороги прибыли.
//! Настройки: money.*, size.*, ограничения исходной биржи.
use super::{
    costs::Calc,
    dirctx::DirCtx,
    dry::Mode,
    engine::{Core, rd},
    settlement_model::{self, Model},
    size::{Curve, Fail, Point},
    types::{Direction, Side},
};
use rust_decimal::Decimal;

struct CashCurve<'a> {
    core: &'a Core,
    dir: &'a Direction,
}
#[async_trait::async_trait]
impl Curve for CashCurve<'_> {
    type Data = Model;
    async fn at(&mut self, qty: Decimal) -> Result<Point<Model>, Fail> {
        let p = settlement_model::point(self.core, self.dir, qty).await?;
        Ok(Point {
            qty,
            invested: p.capital_quote,
            arrive: p.credited_target,
            profit: p.profit_quote,
            data: p,
        })
    }
    fn pct(x: &Model) -> Option<Decimal> {
        x.profit_quote
            .checked_div(x.invested_quote)?
            .checked_mul(Decimal::ONE_HUNDRED)
    }
    fn pct_base(x: &Model) -> Option<Decimal> {
        Some(x.invested_quote)
    }
}

pub(super) fn thresholds(core: &Core, m: &Model, cap: Decimal) -> Result<(), String> {
    let (usd, pct) = super::venue_dir::thresholds(
        &core.inner.deps.settings,
        super::venue_dir::of(core, &m.direction),
    )?;
    if m.capital_quote > cap
        || m.profit_quote <= Decimal::ZERO
        || m.profit_quote < usd
        || CashCurve::pct(m).is_none_or(|x| x < pct)
    {
        return Err(
            "Расчёт с конверсией и всеми расходами не проходит бюджет или порог чистой прибыли"
                .into(),
        );
    }
    Ok(())
}

pub(super) async fn decide(
    core: &Core,
    d: &Direction,
    mode: &Mode,
    cap: Option<Decimal>,
    from: Option<std::time::Instant>,
) -> Result<Calc, String> {
    let cx = DirCtx::for_settlement(core, d)?;
    let market = super::settlement_market::Market::direction(core, d)?;
    let cex = &market.cex;
    if market.cex_symbol() == super::venue_ticker::ticker_on(core, &d.asset, cex) {
        return Err("Целевой и расчётный активы имеют одинаковый тикер биржи".into());
    }
    // Лимиты сделки заданы в валюте учёта. Оценка начального размера берётся с той же биржи в её котируемой валюте.
    let ticker = super::venue_ticker::ticker_on(core, &d.asset, cex);
    let price = rd(&core.inner.market)
        .best(cex, &ticker, Side::Buy)
        .filter(|v| *v > Decimal::ZERO)
        .ok_or("Нет цены целевой монеты на исходной бирже")?;
    let budget = super::limits::budget(core, &cx, mode).await?;
    let budget = cap.map_or(budget, |c| budget.min(c));
    let mut params = super::size_params::params(core, &cx, budget, price, None)?;
    let min =
        super::venue_dir::thresholds(&core.inner.deps.settings, super::venue_dir::of(core, d))?;
    super::size_extra::apply(&core.inner.deps.settings, &mut params, min, from);
    let mut curve = CashCurve { core, dir: d };
    let found = Box::pin(super::size_search::search(&mut curve, &params))
        .await
        .map_err(|e| e.to_string())?;
    let m = crate::gateway::quote_stage::final_scope(settlement_model::evaluate(
        core,
        d,
        found.choice.qty,
    ))
    .await?;
    thresholds(core, &m, budget)?;
    let pct = CashCurve::pct(&m).ok_or("Прибыль вне диапазона")?;
    let mut choice = found.choice;
    choice.why = format!(
        "{} объёмов по полной книге конверсии и целевой сделки",
        choice.evals
    );
    choice.profit = m.profit_quote;
    choice.settlement = Some(Box::new(market));
    Ok(Calc {
        direction: d.clone(),
        units_b_per_a: cx.units_b_per_a,
        transfer_sent_a: Some(m.withdrawal_qty),
        qty: m.buy.qty,
        arrive: m.credited_target,
        sell_qty: m.sell_qty,
        buy_avg: m.buy.avg_price,
        sell_avg: m.sell.avg_price,
        invested: m.invested_quote,
        proceeds: m.proceeds_quote,
        costs: m.cost_lines,
        costs_usdt: m.other_costs_quote + m.reserve_quote,
        profit_usd: m.profit_quote,
        profit_pct: pct,
        transit_s: cx.transit_s,
        sigma: cx.sigma,
        sigma_check: cx.sigma_check,
        buy_quote: m.buy,
        sell_quote: m.sell,
        at: chrono::Utc::now(),
        spend_max: m.capital_quote,
        buy_worst: m.buy_limit,
        buy_band: cx.band_a.is_some(),
        spend_fee: Decimal::ZERO,
        size: Some(choice),
    })
}
