//! Файл: core/credit_rest.rs
//! Что делает: решение 413 (пятое и шестое ревью t198) — остаток «не присвоено» (CreditFact.rest) не теряется и не
//! становится фантомом. По решению 76 счёт биржи — только бота: всё, что на нём сверх учёта других сделок, — монеты
//! сделок бота. Поэтому остаток решается только в ТИХИЙ МОМЕНТ счёта (биржа, монета): у ДРУГИХ сделок нет остатка,
//! ждущих доли, сделок в пути сюда / уходящих отсюда, продаж с неизвестным исходом (remain_others.rs кладёт их в
//! «неясно»). Тогда доля = min(количество + остаток, факт − подтверждённые других) — чтение и запись решения в учёт
//! памяти под одним замком счёта (remain_read::with_fact + credit_lock::sync). Взято больше ожидаемого зачисления —
//! строка «взято сверх зачисления N — остатки прошлых сделок (решение 76)»; меньше количества — Warn и Telegram.
//! - При каждом чтении факта сделки (remain_fact.rs apply_with), на круге сверки — остановленные с остатком (round),
//!   и кнопкой «Продать сейчас» остановленной сделки (manual_halted.rs: не тихо — «счёт занят: …»).
//! - Продажа сделки с остатком кончилась (sell.rs) — закрытия нет: ожидание тихого момента — перечитывание с
//!   интервалом venue.status_poll_ms, не дольше transfer.max_time_s (сделку взял человек — выход); решено — добранное
//!   продаётся (продолжением sell_loop::hop) или сделка закрывается (остатка на счёте нет).
//! - Не задано / предел вышел / любое другое закрытие с остатком (ledger::finish — страховка) — park: сделка
//!   остановлена с подтверждённым количеством и остатком отдельно («не присвоено» — экран сделки, «Ручное управление»),
//!   остаток НЕ подтверждается без факта; Warn и Telegram deal.credit_hold. Решит тихий момент (круг сверки) или
//!   человек: «Продать сейчас» (тихо — присвоить и продать), «Продано вручную» / «Беру на себя» — остаток к человеку.
//!
//! Ожидание держит отметку монеты (coin_ride) сделки — новые сделки этой монеты не начинаются, пока на счёте идут
//! другие сделки (принято: иначе тихий момент не наступит).
//! Отдельная позиция (park) и круг сверки (round) — credit_rest_park.rs.
//! Кто вызывает: core/remain_fact.rs (busy, locked), core/sell_next.rs (at_close), core/credit_rest_park.rs (locked),
//! core/manual_halted.rs (locked).
//! От чего зависит: credit_fact.rs, credit_lock.rs, remain_read.rs (with_fact), remain_others.rs, deposit_hold.rs,
//! credit_alert.rs (wait_ms), ledger.rs, sell_loop.rs, sell_next.rs, deal_words.rs, settings, deps.rs.
//! Настройки: venue.status_poll_ms, transfer.max_time_s (существующие).

use rust_decimal::Decimal;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::deal_words as w;
use super::engine::{Core, lk};
use super::remain_others::Others;

/// Итог решения остатка под замком.
pub enum Res {
    /// Решён (количество обновлено, остатка нет).
    Done,
    /// Счёт не тихий — причина словами.
    Busy(String),
    /// Баланс не прочитан — причина.
    Unread(String),
}

/// Что делает: почему момент счёта не тихий (шапка); None — тихий. Принимает: ядро, сделку, учёт других. Ошибок нет.
pub fn busy(core: &Core, deal: &Deal, o: &Others) -> Option<String> {
    let unsure = super::credit_lock::unsure(core, &deal.at, &deal.direction.asset, &deal.id);
    if o.pending.is_empty() && o.unclear.is_empty() && o.leaving.is_empty() && !unsure {
        return None;
    }
    let mut why = o.words.clone();
    if unsure {
        why.push("продажа другой сделки с неизвестным исходом".into());
    }
    Some(why.join("; "))
}

/// Что делает: решение остатка в тихий момент (чистая часть; вызывать под замком счёта — locked): количество =
/// min(количество + остаток, факт − подтверждённые других). Принимает: сделку, факт, учёт других, ожидаемое удержание
/// %. Возвращает: текст Warn, если вышло меньше количества. Ошибок нет.
pub fn resolve(deal: &mut Deal, fact: Decimal, o: &Others, hold: Decimal) -> Option<String> {
    let rest = super::credit_fact::rest(deal)?;
    let t = deal.direction.asset.ticker.clone();
    let before = deal.qty;
    let qty = (deal.qty + rest).min((fact - o.qty).max(Decimal::ZERO));
    super::credit_fact::confirm(deal, qty, super::credit_fact::Basis::Balance);
    let mut text = format!(
        "Остаток зачисления решён в тихий момент счёта (решение 413): было {}, не присвоено {}, на счёте {}, у других \
         {} — у сделки {}",
        w::coins(before, &t),
        w::coins(rest, &t),
        w::coins(fact, &t),
        w::coins(o.qty, &t),
        w::coins(qty, &t)
    );
    // Ожидаемое зачисление по записи (за вычетом проданного) — сверх него «ничьи» остатки прошлых сделок (решение 76).
    let record = super::credit_fact::here(deal).map_or(Decimal::ZERO, |c| c.record);
    let sold: Decimal = deal.sells.iter().map(|s| s.filled_qty).sum();
    let expected = record * (Decimal::ONE - hold / Decimal::ONE_HUNDRED) - sold;
    if qty > expected && expected >= Decimal::ZERO {
        let over = w::coins(qty - expected, &t);
        text.push_str(&format!(
            "; взято сверх зачисления {over} — остатки прошлых сделок (решение 76)"
        ));
    }
    deal.note(text.clone());
    (qty < before).then(|| {
        format!(
            "{}: {text}; не хватает {}",
            deal.id,
            w::coins(before - qty, &t)
        )
    })
}

/// Что делает: чтение факта и решение остатка под одним замком счёта (шапка). Принимает: ядро, сделку. Возвращает:
/// Res. Ошибок нет.
pub async fn locked(core: &Core, deal: &mut Deal) -> Res {
    let probe = deal.clone();
    let hold = super::deposit_hold::expected_pct(core, &deal.at, &deal.direction.asset);
    let mut short = None;
    let res = Box::pin(super::remain_read::with_fact(core, &probe, |r, o| {
        match (busy(core, deal, &o), r) {
            (Some(why), _) => Res::Busy(why),
            (None, Err(e)) => Res::Unread(e),
            (None, Ok(fact)) => {
                short = resolve(deal, fact, &o, hold);
                super::credit_lock::sync(core, deal);
                Res::Done
            }
        }
    }))
    .await;
    if let Some(t) = short {
        let deps = &core.inner.deps;
        Box::pin(deps.log(Level::Warn, "core.credit_fact", Some(&deal.at.0), &t, None)).await;
        Box::pin(deps.notify("deal.credit_hold", Some(&deal.at.0), &t)).await;
    }
    res
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

/// Итог ожидания тихого момента.
enum Waited {
    Done,
    Timeout,
    Gone,
}

/// Ожидание тихого момента и решение (шапка). Ошибок нет.
async fn wait_quiet(core: &Core, deal: &mut Deal) -> Waited {
    let Some((ms, max)) = pace(core, deal) else {
        return Waited::Timeout;
    };
    let mark = lk(&core.inner.deals)
        .get(&deal.id)
        .map(|d| (d.step, d.updated_at));
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(max);
    loop {
        if let Res::Done = Box::pin(locked(core, deal)).await {
            return Waited::Done;
        }
        if tokio::time::Instant::now() >= until {
            return Waited::Timeout;
        }
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        // Шестое ревью (М-3): сделку взял человек (её запись в учёте изменилась) — ожидание кончается.
        if lk(&core.inner.deals)
            .get(&deal.id)
            .map(|d| (d.step, d.updated_at))
            != mark
        {
            return Waited::Gone;
        }
    }
}

/// Что делает: продажа сделки с остатком кончилась — ждём тихий момент (шапка). Принимает: ядро, сделку, причину
/// продажи. Ошибок нет.
pub async fn at_close(core: &Core, mut deal: Deal, why: String) {
    match Box::pin(wait_quiet(core, &mut deal)).await {
        Waited::Gone => {}
        Waited::Timeout => {
            let t =
                "тихий момент счёта не наступил (не задано или вышел предел transfer.max_time_s)";
            Box::pin(super::credit_rest_park::park(core, &mut deal, t)).await
        }
        Waited::Done if deal.qty > Decimal::ZERO => {
            // Добранное продаёт автоматика, как остаток частичной продажи (решение 177) — продолжением цикла.
            Box::pin(super::ledger::save(core, &deal)).await;
            let left = super::stuck_reason::StuckReason::Partial(deal.qty);
            super::sell_loop::hop(super::sell_next::stuck(core, deal, left, true)).await
        }
        Waited::Done => {
            let t = format!("продано ({why}); остатка зачисления на счёте нет");
            Box::pin(super::ledger::finish(core, &mut deal, Step::Closed, &t)).await
        }
    }
}
