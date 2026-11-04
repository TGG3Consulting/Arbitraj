//! Файл: core/precheck_withdraw.rs
//! Что делает: предпроверка до покупки (решение 150, Тигран 04.11.2026): хватит ли на бирже A средств, которые МОЖНО
//! ВЫВЕСТИ. Живой случай 04.11 (сделка 20261104173851521BABYDOGE, mexc → bsc_dex): покупка на MEXC прошла, вывод —
//! отказ «код 16021: Some pre-crediting assets are temporarily unavailable for withdrawal». Предварительно зачисленные
//! депозиты торгуются, но не выводятся; как именно MEXC ограничивает вывод, в документации нет. Строгое правило
//! Тиграна до ответа MEXC: из того, что можно вывести, вычитается ВСЯ стоимость удержаний в USDT, в какой бы монете они
//! ни были: можно вывести = свободная котируемая монета на A − удержания в USDT; меньше суммы покупки — сделки нет.
//! Проверка — только если A биржа (после покупки будет вывод с неё). Удержаний нет — ничего не меняет; у бирж без
//! такого механизма контракт отвечает пусто без запросов. Сбой запроса удержаний или нет цены удержания — сделки нет.
//! Сумма покупки — Calc::spend_max (наибольшее списание плана, не больше бюджета сделки; решение 66).
//! Кто вызывает: core/precheck.rs (после пересчёта с финальной котировкой).
//! От чего зависит: trade_ops.rs (вид площадки), limits.rs (free_now), convert.rs (price_of), deps.rs (шлюз).
//! Настройки: venue.quote_asset (по площадке A); новых нет.

use rust_decimal::Decimal;

use crate::core::types::{Direction, VenueId, WithdrawHold};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::costs::Calc;
use super::engine::{Core, rd};

/// Что делает: проверка «можно вывести» на бирже A перед покупкой. Принимает: ядро, направление, свежий расчёт.
/// Возвращает: Ok — A не биржа, удержаний нет или выводимого хватает. Ошибки: текст причины — сбой запроса удержаний,
/// нет цены удержания, не задана venue.quote_asset, сбой баланса, выводимого меньше суммы покупки.
pub async fn check(core: &Core, d: &Direction, plan: &Calc) -> Result<(), String> {
    let a = &d.buy_on;
    if super::trade_ops::kind(core, a)? != VenueKind::Cex {
        return Ok(());
    }
    let gw = core.inner.deps.gw.get(a).await.map_err(|e| e.message)?;
    let hold = gw.withdraw_hold().await.map_err(|e| {
        format!(
            "{}: удержания вывода не получены (история депозитов): {} — не торгуем вслепую",
            a.0, e.message
        )
    })?;
    if hold.is_empty() {
        return Ok(());
    }
    let at = At {
        venue: Some(&a.0),
        coin: None,
    };
    let quote = core
        .inner
        .deps
        .settings
        .text("venue.quote_asset", at)
        .map_err(|e| e.message)?;
    let held = held_usdt(core, a, &quote, &hold).await?;
    let free = super::limits::free_now(core, a, None, &quote).await?;
    verdict(&a.0, &quote, free, held, plan.spend_max, &reasons(&hold))
}

/// Что делает: стоимость всех удержаний в USDT. Котируемая монета A — как есть; другая — по живому стакану биржи
/// (convert::price_of — как у оценки монет в USDT, credit_event.rs): сначала A, затем остальные биржи.
/// Ошибки: текст — у удержания нет живой цены (неизвестная стоимость = сделки нет).
async fn held_usdt(
    core: &Core,
    a: &VenueId,
    quote: &str,
    hold: &WithdrawHold,
) -> Result<Decimal, String> {
    let mut cex = vec![a.clone()];
    cex.extend(
        rd(&core.inner.kinds)
            .iter()
            .filter(|(v, k)| *k == VenueKind::Cex && v != a)
            .map(|(v, _)| v.clone()),
    );
    let mut sum = Decimal::ZERO;
    for h in &hold.items {
        if h.asset == quote {
            sum += h.amount;
            continue;
        }
        let p = super::convert::price_of(&core.inner.deps, &cex, &h.asset, h.amount)
            .await
            .map_err(|e| {
                format!(
                    "{}: {} — стоимость в USDT неизвестна ({e}), вывод может не пройти",
                    a.0, h.reason
                )
            })?;
        sum += h.amount * p;
    }
    Ok(sum)
}

/// Тексты удержаний через «; ». Ошибок нет.
fn reasons(hold: &WithdrawHold) -> String {
    let parts: Vec<&str> = hold.items.iter().map(|h| h.reason.as_str()).collect();
    parts.join("; ")
}

/// Что делает: решение по числам (чистая функция). Принимает: площадку, котируемую монету, свободно на A, удержания в
/// USDT, сумму покупки, тексты удержаний. Возвращает: Ok — удержаний нет (held ≤ 0) или «можно вывести» не меньше
/// покупки. Ошибки: текст «mexc: предварительно зачислено … — вывод не пройдёт: можно вывести …».
pub fn verdict(
    venue: &str,
    quote: &str,
    free: Decimal,
    held: Decimal,
    spend: Decimal,
    why: &str,
) -> Result<(), String> {
    if held <= Decimal::ZERO {
        return Ok(());
    }
    let can = free - held;
    if can >= spend {
        return Ok(());
    }
    Err(format!(
        "{venue}: {why} — вывод не пройдёт: можно вывести {free} − {} = {} {quote} меньше покупки {} {quote}",
        held.round_dp(4),
        can.round_dp(4),
        spend.round_dp(4)
    ))
}
