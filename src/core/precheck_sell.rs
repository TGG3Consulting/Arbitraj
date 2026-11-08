//! Файл: core/precheck_sell.rs
//! Что делает: предпроверка стороны ПРОДАЖИ до покупки (решение 134, «обе стороны до покупки»): биржа B примет наш
//! ордер продажи на объём, который придёт на неё по плану (Calc::sell_qty — приход после налогов и комиссий вниз до шага
//! пары B), по худшей цене, которую поставит исполнение (worst.rs). Проверяет адаптер B (контракт check_order): те же
//! правила, что у его настоящего ордера (тип ордера, пара торгуется через API в эту сторону, шаги, минимумы и
//! максимумы), и пробный ордер площадки, если он есть и включён (MEXC — venue.order_test_first). Отказ — сделки нет,
//! денег не тронуто. Живой случай 03.11 (сделка 20261103201513697WKC, bsc_dex → mexc): продажа на MEXC отклонена по
//! типу ордера уже после покупки и перевода — монета застряла, продана вручную, −7.98 USDT.
//! DEX-сторона продажи (решение 234) — симуляция настоящей продажи кошельком сделки и пересчёт по её выходу:
//! precheck_dex_sell.rs (живой случай ASS 08.11: продажа откатывалась «minOut» уже после покупки и вывода).
//! Отказ НЕ считается ошибкой площадки (risk.max_errors_in_row): это отказ до сделки, как прочие отказы предпроверки.
//! Кто вызывает: core/precheck.rs (после пересчёта с финальной котировкой).
//! От чего зависит: precheck_dex_sell.rs (DEX), worst.rs (худшая цена), trade_ops.rs (вид площадки), deps.rs (шлюз), costs_types.rs (Calc).
//! Настройки: через worst.rs и адаптер B (venue.order_type, venue.order_time_in_force, venue.order_test_first).

use rust_decimal::Decimal;

use crate::core::types::{Direction, Side, TradeRequest};
use crate::gateway::contract::VenueKind;

use super::costs::Calc;
use super::engine::Core;

/// Что делает: проверка продажи на B по плану. Принимает: ядро, направление, свежий расчёт (plan).
/// Возвращает: расчёт для сделки — у биржи B тот же (биржа примет ордер), у DEX — пересчитанный по симуляции продажи
/// (решение 234, precheck_dex_sell.rs). Ошибки: текст «продажа на B не пройдёт: …» / «невыгодна по симуляции …».
pub async fn check(core: &Core, d: &Direction, plan: Calc) -> Result<Calc, String> {
    let b = &d.sell_on;
    if super::trade_ops::kind(core, b)? != VenueKind::Cex {
        return super::precheck_dex_sell::check(core, d, plan).await;
    }
    let fail = |why: String| format!("продажа на {} не пройдёт: {why}", b.0);
    let req = request(core, d, &plan).map_err(fail)?;
    let gw = core
        .inner
        .deps
        .gw
        .get(b)
        .await
        .map_err(|e| fail(e.message))?;
    gw.check_order(&req)
        .await
        .map_err(|e| fail(format!("{} ({:?})", e.message, e.code)))?;
    Ok(plan)
}

/// Что делает: ордер продажи по плану — как его поставит исполнение (trade_ops.rs): объём — plan.sell_qty, худшая
/// цена — worst.rs по средней цене котировки продажи; маршрут DEX — из котировки продажи плана (решение 234; у биржи
/// маршрута нет). Принимает: ядро, направление, расчёт. Возвращает: запрос.
/// Ошибки: текст — объём продажи не больше нуля, нет стакана или глубины на объём.
pub fn request(core: &Core, d: &Direction, plan: &Calc) -> Result<TradeRequest, String> {
    let qty = plan.sell_qty;
    if qty <= Decimal::ZERO {
        return Err(format!(
            "объём продажи по плану {qty} {} не больше нуля",
            d.asset.ticker
        ));
    }
    let worst = super::worst::worst_price(
        core,
        &d.sell_on,
        &d.asset,
        Side::Sell,
        qty,
        plan.sell_quote.avg_price,
    )?;
    Ok(TradeRequest {
        venue: d.sell_on.clone(),
        // Решение 211: ордер площадки — с её тикером монеты.
        asset: super::venue_ticker::on_venue(core, &d.asset, &d.sell_on),
        side: Side::Sell,
        qty,
        worst_price: worst,
        // Сделки ещё нет (ID выдаёт учёт после предпроверки): для своего ID пробного ордера — метка «precheck».
        deal_id: "precheck".into(),
        route: plan.sell_quote.route.clone(),
        // Решение 177: та же продажа выхода, что поставит исполнение (trade_ops.rs).
        exit_position: true,
    })
}
