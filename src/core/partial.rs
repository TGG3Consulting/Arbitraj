//! Файл: core/partial.rs
//! Что делает: частичная покупка (решение 48) — обычным полным расчётом со всеми затратами сравниваются
//! два итога сделки: «довезти и продать, как задумано» (costs.rs на фактический объём, с фактической
//! ценой покупки вместо котировки) и «продать обратно на площадке покупки» (trade_ops::sell_value).
//! Выбирается больший итог (оба в минусе — меньший убыток). Режим застрявшей монеты здесь не включается.
//! Решение 240: контекст — DirCtx::build_held (сделка уже идёт): как до решения 240, без доли закрытия разницы, границы
//! σ и проверки profit.sigma_min_changes.
//! Решение 464 (t238, ревью): «довезти» не проигрывает «продать на A» из-за нашей же продажи монеты на B (ордер в
//! полёте — ждём его итога, это время ордера) — только при этой причине отказа (sale_marks::MARK); прочие отказы —
//! прежний выбор. Решение 480 (Тигран 21.11, меняет ожидание разбора 464): исход продажи другой сделки Z на B
//! неизвестен — «довезти» НЕ ждёт: расчёт продажи на B считает остаток Z как ещё не исполненную (худший случай,
//! ride_quote::ahead), и выбор «довезти» / «продать на A» делается сразу по нему; «продать на A» — с тем же худшим
//! случаем для наших продаж на A (partial_back.rs); строки из расчёта — в хронологию, уведомление
//! deal.partial_waits_review (Z надо разобрать, сделка не ждёт).
//! Кто вызывает: core/executor.rs.
//! От чего зависит: dirctx.rs, costs.rs, partial_back.rs («продать на A»), deal.rs, sale_marks.rs.
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
/// площадке продажи (по причине — метка sale_marks::MARK в тексте отказа ride_settled.rs) — не повод продавать на A:
/// ордер в полёте или кончился за время расчёта — ждём его конца (время ордера) и считаем снова. Решение 480: продажа
/// другой сделки с неизвестным исходом на B ожидания не даёт — расчёт уже считает её худшим случаем. Другой отказ —
/// прежний выбор. Принимает: ядро, сделку. Возвращает: (итог, строка худшего случая из расчёта — Calc.worst_b).
/// Ошибки: текст.
async fn deliver_value(core: &Core, deal: &Deal) -> Valued {
    let (v, a) = (&deal.direction.sell_on, &deal.direction.asset);
    loop {
        // Будильник — до расчёта: конец ордера во время расчёта не теряется.
        let wake = core.inner.sales.done.notified();
        let r = match super::dirctx::DirCtx::build_held(core, &deal.direction) {
            Ok(cx) => super::costs::evaluate(core, &cx, deal.qty)
                .await
                .map(|c| (deliver_result(&c, deal), c.worst_b))
                .map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        if !matches!(&r, Err(e) if e.contains(super::sale_marks::MARK)) {
            return r;
        }
        // Наш ордер в полёте — ждём его конца; кончился за время расчёта — сразу считаем снова (по пулу после него).
        if super::sale_marks::busy(core, v, a) {
            wake.await;
        }
    }
}

/// Итог пути с строкой худшего случая из его расчёта (решение 480) или причина отказа.
type Valued = Result<(Decimal, Option<String>), String>;

/// Строка худшего случая пути (None — не было или путь не посчитан). Ошибок нет.
fn line_of(r: &Valued) -> Option<String> {
    r.as_ref().ok().and_then(|x| x.1.clone())
}

/// Решение 480: строки худшего случая обоих путей (из их расчётов, набор не пересобирается) — в хронологию; человеку —
/// уведомление WAITS с теми же строками (сделка не ждёт). Принимает: ядро, сделку, строки «довезти» и «продать на A».
/// Ошибок нет.
async fn worst_notes(core: &Core, deal: &mut Deal, notes: [Option<String>; 2]) {
    let mut lines = Vec::new();
    for (path, line) in ["«довезти»", "«продать на A»"].iter().zip(notes) {
        if let Some(line) = line {
            deal.note(format!("Частичная покупка, {path}: {line}"));
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return;
    }
    let (at, text) = (deal.direction.buy_on.0.clone(), waits_text(deal, &lines));
    core.inner.deps.notify(WAITS, Some(&at), &text).await;
}

/// Событие уведомления (имя прежнее): пути частичной покупки посчитаны с худшим случаем по сделке с неизвестным исходом
/// продажи на B или A (решение 480: сделка не ждёт).
pub const WAITS: &str = "deal.partial_waits_review";

/// Текст уведомления WAITS для человека. Принимает: сделку, строки худшего случая из расчётов. Ошибок нет.
pub fn waits_text(deal: &Deal, lines: &[String]) -> String {
    let (id, q, d) = (&deal.id, deal.qty, &deal.direction);
    let (t, a) = (&d.asset.ticker, &d.buy_on.0);
    format!(
        "Сделка {id} (частичная покупка: {q} {t} на {a}) не ждёт: «довезти» и «продать на {a}» посчитаны сразу. {}. \
         Разберите сделки с неизвестным исходом продажи — «Ручное управление»: «Продать сейчас», «Продано вручную» \
         или «Беру на себя»",
        lines.join("; ")
    )
}

/// Выбор пути. Принимает: ядро, сделку после частичной покупки. Возвращает: Choice. Ошибок нет.
pub async fn choose(core: &Core, deal: &mut Deal) -> Choice {
    let deliver = Box::pin(deliver_value(core, deal)).await;
    // Решение 220: котировка продажи на A — кошельком сделки (им пойдёт продажа). Решение 480 (ревью): с тем же худшим
    // случаем, что у B — наши продажи на A с неизвестным исходом впереди (partial_back.rs).
    let back = match super::dex_account::of(core, deal) {
        Ok(acc) => Box::pin(super::partial_back::value(core, acc.as_ref(), deal))
            .await
            .map(|(v, n)| (deal.result_if(v), n)),
        Err(e) => Err(e),
    };
    worst_notes(core, deal, [line_of(&deliver), line_of(&back)]).await;
    match (deliver.map(|x| x.0), back.map(|x| x.0)) {
        (Ok(d), Ok(b)) if d >= b => Choice::Deliver(d),
        (Ok(_), Ok(b)) => Choice::SellBack(b),
        (Ok(d), Err(_)) => Choice::Deliver(d),
        (Err(_), Ok(b)) => Choice::SellBack(b),
        (Err(d), Err(b)) => Choice::Neither(format!("довезти: {d}; продать на A: {b}")),
    }
}
