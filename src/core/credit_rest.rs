//! Файл: core/credit_rest.rs
//! Что делает: решение 413 (пятое ревью t198) — остаток «не присвоено» (CreditFact.rest) не теряется. По решению 76
//! счёт биржи — только бота: всё, что на нём есть сверх учёта других сделок, — монеты сделок бота. Поэтому остаток
//! решается в ТИХИЙ МОМЕНТ счёта (биржа, монета): нет других ждущих доли, нет сделок в пути сюда / уходящих отсюда, нет
//! продаж с неизвестным исходом (Deal.sale_unsure) — тогда доля = min(количество + остаток, факт − подтверждённые
//! других): точно, без снимка и без истории депозитов.
//! - При каждом чтении факта сделки (remain_fact.rs apply_with) — если момент тихий, остаток решается сразу.
//! - Продажа сделки с остатком кончилась (sell.rs, «продано») — закрытия нет: ожидание тихого момента — перечитывание
//!   под замком счёта с интервалом venue.status_poll_ms, не дольше transfer.max_time_s; решено — добранное продаётся
//!   (продолжением sell_loop::hop) или сделка закрывается, если добирать нечего.
//! - Не задано / предел вышел / любое другое закрытие с остатком (ledger::finish — страховка) — основная часть
//!   продана, остаток остаётся отдельной видимой позицией сделки: сделка остановлена с количеством = остаток (экран
//!   сделки, «Ручное управление»: «Продать сейчас» — присвоить и продать по факту счёта под замком, «Беру на себя» /
//!   «Продано вручную» — оставить), журнал core.credit_fact Warn и Telegram deal.credit_hold с цифрами; в итоге дня —
//!   оценкой, как остановленные (решения 53, 152).
//!
//! Кто вызывает: core/remain_fact.rs (quiet, resolve), core/sell_next.rs (at_close), core/ledger.rs (park).
//! От чего зависит: credit_fact.rs, credit_lock.rs (unsure), remain_read.rs, remain_others.rs, credit_alert.rs
//! (wait_ms), ledger.rs, sell.rs, sell_loop.rs, deal_words.rs, settings, deps.rs.
//! Настройки: venue.status_poll_ms, transfer.max_time_s (существующие).

use rust_decimal::Decimal;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::deal_words as w;
use super::engine::Core;
use super::remain_others::Others;

/// Что делает: тихий момент счёта сделки (шапка). Принимает: ядро, сделку, учёт других. Ошибок нет.
pub fn quiet(core: &Core, deal: &Deal, o: &Others) -> bool {
    o.pending.is_empty()
        && o.unclear.is_empty()
        && o.leaving.is_empty()
        && !super::credit_lock::unsure(core, &deal.at, &deal.direction.asset)
}

/// Что делает: решает остаток в тихий момент: количество = min(количество + остаток, факт − подтверждённые других),
/// остатка больше нет. Принимает: сделку, факт, учёт других. Возвращает: да — был остаток и решён. Ошибок нет.
pub fn resolve(deal: &mut Deal, fact: Decimal, o: &Others) -> bool {
    let Some(rest) = super::credit_fact::rest(deal) else {
        return false;
    };
    let before = deal.qty;
    let qty = (deal.qty + rest).min((fact - o.qty).max(Decimal::ZERO));
    super::credit_fact::confirm(deal, qty, super::credit_fact::Basis::Balance);
    let t = &deal.direction.asset.ticker;
    deal.note(format!(
        "Остаток зачисления решён в тихий момент счёта (решение 413, решение 76: счёт — только бота): было {}, \
         не присвоено {}, на счёте {}, у других {} — у сделки {}",
        w::coins(before, t),
        w::coins(rest, t),
        w::coins(fact, t),
        w::coins(o.qty, t),
        w::coins(qty, t)
    ));
    true
}

/// (интервал мс, предел с) ожидания тихого момента; None — не задано. Ошибок нет.
fn pace(core: &Core, deal: &Deal) -> Option<(u64, u64)> {
    let a = &deal.direction.asset;
    let coin = crate::settings::scope::coin_id(&a.ticker, a.bsc_contract.as_ref());
    let at = At {
        venue: Some(&deal.at.0),
        coin: Some(&coin),
    };
    let max = core
        .inner
        .deps
        .settings
        .integer("transfer.max_time_s", at)
        .ok()?;
    Some((super::credit_alert::wait_ms(core, &deal.at)?, max))
}

/// Что делает: ждёт тихий момент и решает остаток (шапка). Принимает: ядро, сделку. Возвращает: да — решён; нет — не
/// задано или предел вышел. Ошибок нет.
pub async fn wait_quiet(core: &Core, deal: &mut Deal) -> bool {
    let Some((ms, max)) = pace(core, deal) else {
        return false;
    };
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(max);
    loop {
        let (r, o) = Box::pin(super::remain_read::read_with(core, deal, false)).await;
        if let Ok(fact) = r
            && quiet(core, deal, &o)
        {
            return resolve(deal, fact, &o);
        }
        if tokio::time::Instant::now() >= until {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
}

/// Что делает: продажа сделки с остатком кончилась — не закрываем: ждём тихий момент; решено — продаём добранное
/// (продолжением) или закрываем; нет — остаток отдельной позицией (park). Принимает: ядро, сделку, причину продажи.
/// Ошибок нет.
pub async fn at_close(core: &Core, mut deal: Deal, why: String) {
    if !Box::pin(wait_quiet(core, &mut deal)).await {
        let t = "тихий момент счёта не наступил (не задано или вышел предел transfer.max_time_s)";
        return Box::pin(park(core, &mut deal, t)).await;
    }
    if deal.qty > Decimal::ZERO {
        // Добранное продаёт автоматика, как остаток частичной продажи (решение 177) — продолжением цикла.
        super::ledger::save(core, &deal).await;
        let left = super::stuck_reason::StuckReason::Partial(deal.qty);
        return super::sell_loop::hop(super::sell_next::stuck(core, deal, left, true)).await;
    }
    Box::pin(super::ledger::finish(
        core,
        &mut deal,
        Step::Closed,
        &format!("продано ({why})"),
    ))
    .await
}

/// Что делает: остаток — отдельной видимой позицией (шапка): сделка остановлена с количеством = остаток, журнал Warn и
/// Telegram deal.credit_hold с цифрами. Принимает: ядро, сделку, причину. Ошибок нет.
pub async fn park(core: &Core, deal: &mut Deal, why: &str) {
    let Some(rest) = super::credit_fact::rest(deal) else {
        return Box::pin(super::ledger::finish(core, deal, Step::Halted, why)).await;
    };
    let t = deal.direction.asset.ticker.clone();
    let record = super::credit_fact::here(deal).map_or(Decimal::ZERO, |c| c.record);
    let sold: Decimal = deal.sells.iter().map(|s| s.filled_qty).sum();
    let text = format!(
        "{}: остаток зачисления не решён — {why}. Запись депозита {}, доля {}, продано {}, не присвоено {} — остаётся \
         отдельной позицией сделки (остановлена): «Продать сейчас» — присвоить и продать по факту счёта, «Беру на себя» \
         или «Продано вручную» — оставить (решение 413)",
        w::deal(core, deal),
        w::coins(record, &t),
        w::coins(deal.qty + sold, &t),
        w::coins(sold, &t),
        w::coins(rest, &t)
    );
    super::credit_fact::confirm(deal, deal.qty + rest, super::credit_fact::Basis::Balance);
    let deps = &core.inner.deps;
    Box::pin(deps.log(
        Level::Warn,
        "core.credit_fact",
        Some(&deal.at.0),
        &text,
        None,
    ))
    .await;
    Box::pin(deps.notify("deal.credit_hold", Some(&deal.at.0), &text)).await;
    Box::pin(super::ledger::finish(core, deal, Step::Halted, &text)).await
}
