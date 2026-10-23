//! Файл: core/profit.rs
//! Что делает: решение по кандидату (шаг 4 ТЗ): подбирает объём и проверяет прибыль после всех затрат.
//! Объёмы-кандидаты — границы уровней стакана биржи на стороне A (asks) и B (bids) в пределах бюджета
//! (limits.rs) плюс сам бюджет; объём наращивается, пока прибыль растёт (ТЗ: «пока следующая порция
//! ещё прибыльна»). Сделка проходит, если прибыль ≥ money.min_profit_usd и ≥ money.min_profit_pct
//! от вложенного. Иначе — отказ с причиной (для подбора порогов).
//! Кто вызывает: core/candidates.rs (очередь тяжёлого расчёта), позже executor.rs (предпроверка).
//! От чего зависит: costs.rs, dirctx.rs, limits.rs, market_state.rs, settings.
//! Настройки: money.min_profit_usd, money.min_profit_pct (+ всё, что читают costs.rs, dirctx.rs, limits.rs).

use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{Direction, Side};
use crate::settings::read::At;

use super::costs::{self, Calc};
use super::dirctx::DirCtx;
use super::engine::{Core, rd};

/// Итог расчёта кандидата.
#[derive(Debug, Clone, Serialize)]
pub enum Verdict {
    /// Прибыль выше порога — сделка.
    Accept(Calc),
    /// Отказ: причина и лучший найденный расчёт (если был).
    Reject { reason: String, best: Option<Calc> },
}

/// Объёмы для перебора, по возрастанию: границы уровней стаканов бирж сделки до бюджета + бюджет.
/// Принимает: ядро, направление, бюджет в USDT, опорную цену покупки. Ошибок нет.
fn sizes(core: &Core, d: &Direction, budget: Decimal, price: Decimal) -> Vec<Decimal> {
    let max = budget / price;
    let market = rd(&core.inner.market);
    let mut out = vec![max];
    for (venue, side) in [(&d.buy_on, Side::Buy), (&d.sell_on, Side::Sell)] {
        if let Some(b) = market.book(venue, &d.asset.ticker) {
            let levels = if side == Side::Buy { &b.asks } else { &b.bids };
            let mut cum = Decimal::ZERO;
            for l in levels {
                cum += l.qty;
                if cum >= max {
                    break;
                }
                out.push(cum);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Расчёт кандидата. Принимает: ядро, направление. Возвращает: Verdict. Ошибок нет — всё в Reject.
pub async fn decide(core: &Core, d: &Direction) -> Verdict {
    let reject = |reason: String, best: Option<Calc>| Verdict::Reject { reason, best };
    let cx = match DirCtx::build(core, d) {
        Ok(c) => c,
        Err(e) => return reject(e, None),
    };
    let s = &core.inner.deps.settings;
    let (min_usd, min_pct) = match (
        s.decimal("money.min_profit_usd", At::default()),
        s.decimal("money.min_profit_pct", At::default()),
    ) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return reject(e.message, None),
    };
    let budget = match super::limits::budget(core, &cx).await {
        Ok(b) => b,
        Err(e) => return reject(e, None),
    };
    let price = rd(&core.inner.market).best(&d.buy_on, &d.asset.ticker, Side::Buy);
    let price = match price.or(rd(&core.inner.spreads)
        .get(d)
        .and_then(|v| v.prices.as_ref().map(|p| p.buy)))
    {
        Some(p) if !p.is_zero() => p,
        _ => return reject("нет цены покупки на A".into(), None),
    };
    let mut best: Option<Calc> = None;
    let mut last_err = String::new();
    for q in sizes(core, d, budget, price) {
        match costs::evaluate(&core.inner.deps, &cx, q).await {
            Ok(c) if c.invested > budget => {
                last_err = format!("объём {q}: вложение {} больше бюджета {budget}", c.invested);
                break;
            }
            Ok(c) => {
                let better = best.as_ref().is_none_or(|b| c.profit_usd > b.profit_usd);
                if !better && best.is_some() {
                    break; // прибыль перестала расти — дальше объём не наращиваем
                }
                best = Some(c);
            }
            // Мелкий объём может не пройти минимум вывода или депозита — пробуем больше.
            Err(e) if best.is_none() => last_err = e,
            Err(e) => {
                last_err = e;
                break;
            }
        }
    }
    match best {
        None => reject(last_err, None),
        Some(c) if c.profit_usd < min_usd => reject(
            format!(
                "прибыль {} USDT ниже порога {min_usd}",
                c.profit_usd.round_dp(4)
            ),
            Some(c),
        ),
        Some(c) if c.profit_pct < min_pct => reject(
            format!(
                "прибыль {}% ниже порога {min_pct}%",
                c.profit_pct.round_dp(4)
            ),
            Some(c),
        ),
        Some(c) => Verdict::Accept(c),
    }
}
