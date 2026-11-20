//! Файл: core/partial.rs
//! Что делает: частичная покупка (решение 48) — обычным полным расчётом со всеми затратами сравниваются
//! два итога сделки: «довезти и продать, как задумано» (costs.rs на фактический объём, с фактической
//! ценой покупки вместо котировки) и «продать обратно на площадке покупки» (trade_ops::sell_value).
//! Выбирается больший итог (оба в минусе — меньший убыток). Режим застрявшей монеты здесь не включается.
//! Решение 240: контекст — DirCtx::build_held (сделка уже идёт): как до решения 240, без доли закрытия разницы, границы
//! σ и проверки profit.sigma_min_changes.
//! Решение 464 (t238, ревью): «довезти» не проигрывает «продать на A» из-за нашей же продажи монеты на B (ордер в
//! полёте — ждём его итога; исход продажи другой сделки неизвестен — ждём её разбора, строка в хронологии) —
//! только при этой причине отказа (sale_marks::MARK); прочие отказы — прежний выбор.
//! Кто вызывает: core/executor.rs.
//! От чего зависит: dirctx.rs, costs.rs, trade_ops.rs, deal.rs, sale_marks.rs.
//! Настройки: через costs.rs и dirctx.rs.
//!
//! Осторожность расчёта: затраты покупки в монете из котировки уже вычтены из факта (deal.qty),
//! а расчёт «довезти» вычитает их ещё раз — итог «довезти» слегка занижен, но не завышен.

use rust_decimal::Decimal;

use super::costs::Calc;
use super::deal::Deal;
use super::engine::Core;

/// Что делать с купленным частично.
#[derive(Debug)]
pub enum Choice {
    /// Довезти до B и продать — ожидаемый итог сделки, USDT.
    Deliver(Decimal),
    /// Продать на A — ожидаемый итог сделки, USDT.
    SellBack(Decimal),
    /// Ни один путь не считается — причины обоих.
    Neither(String),
}

/// Итог «довезти»: выручка расчёта минус затраты расчёта без затрат покупки (их заменяет факт),
/// минус фактически вложенное и потраченное. Принимает: расчёт, сделку.
fn deliver_result(c: &Calc, deal: &Deal) -> Decimal {
    let buy_costs: Decimal = c
        .costs
        .iter()
        .filter(|l| l.stage == "A" && !l.in_price && !l.in_qty)
        .filter_map(|l| l.usdt)
        .sum();
    c.proceeds - (c.costs_usdt - buy_costs) - deal.invested - deal.spent
}

/// Итог «довезти» (расчёт на фактический объём). Решение 464 (t238, ревью): отказ ИЗ-ЗА НАШЕЙ продажи этой монеты на
/// площадке продажи (по причине — метка sale_marks::MARK в тексте отказа ride_settled.rs / sale_marks.rs) — не повод
/// продавать на A: ордер в полёте или кончился за время расчёта — ждём его конца и считаем снова; исход продажи другой
/// сделки неизвестен — ждём разбора той сделки (строка «ждём разбора сделки Z» — в хронологию). Другой отказ — прежний
/// выбор. Принимает: ядро, сделку. Ошибки: текст причины.
async fn deliver_value(core: &Core, deal: &mut Deal) -> Result<Decimal, String> {
    let (v, a) = (deal.direction.sell_on.clone(), deal.direction.asset.clone());
    let mut noted: Option<String> = None;
    loop {
        // Будильник — до расчёта: конец ордера или снятие блокировки во время расчёта не теряются.
        let wake = core.inner.sales.done.notified();
        let r = match super::dirctx::DirCtx::build_held(core, &deal.direction) {
            Ok(cx) => super::costs::evaluate(core, &cx, deal.qty)
                .await
                .map(|c| deliver_result(&c, deal))
                .map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        let e = match r {
            Ok(x) => return Ok(x),
            Err(e) => e,
        };
        if !e.contains(super::sale_marks::MARK) {
            return Err(e);
        }
        if let Some(id) = super::sale_marks::unsure_of(core, &v, &a) {
            if noted.as_deref() != Some(id.as_str()) {
                deal.note(format!(
                    "Частичная покупка: «довезти» ждёт разбора сделки {id} — исход её продажи {} на {} неизвестен \
                     (решение 464)",
                    a.ticker, v.0
                ));
                super::ledger::save(core, deal).await;
                noted = Some(id);
            }
            wake.await;
        } else if super::sale_marks::busy(core, &v, &a) {
            wake.await;
        }
        // Иначе наш ордер кончился за время расчёта — сразу считаем снова (по пулу после него).
    }
}

/// Выбор пути. Принимает: ядро, сделку после частичной покупки. Возвращает: Choice. Ошибок нет.
pub async fn choose(core: &Core, deal: &mut Deal) -> Choice {
    let deliver = Box::pin(deliver_value(core, deal)).await;
    let deal = &*deal;
    // Решение 220: котировка продажи на A — кошельком сделки (им пойдёт продажа).
    let back = match super::dex_account::of(core, deal) {
        Ok(acc) => super::trade_ops::sell_value(
            core,
            acc.as_ref(),
            &deal.direction.buy_on,
            &deal.direction.asset,
            deal.qty,
        )
        .await
        .map(|(v, _)| deal.result_if(v)),
        Err(e) => Err(e),
    };
    match (deliver, back) {
        (Ok(d), Ok(b)) if d >= b => Choice::Deliver(d),
        (Ok(_), Ok(b)) => Choice::SellBack(b),
        (Ok(d), Err(_)) => Choice::Deliver(d),
        (Err(_), Ok(b)) => Choice::SellBack(b),
        (Err(d), Err(b)) => Choice::Neither(format!("довезти: {d}; продать на A: {b}")),
    }
}
