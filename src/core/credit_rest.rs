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
//! Ожидание после продажи (at_close), отдельная позиция (park) и круг сверки (round) — credit_rest_park.rs.
//! Кто вызывает: core/remain_fact.rs (busy, locked), core/credit_rest_park.rs (locked), core/manual_halted.rs и
//! core/manual.rs (locked — кнопки продажи).
//! От чего зависит: credit_fact.rs, credit_lock.rs, remain_read.rs (with_fact), remain_others.rs, deposit_hold.rs,
//! credit_alert.rs (wait_ms), ledger.rs, sell_loop.rs, sell_next.rs, deal_words.rs, settings, deps.rs.
//! Настройки: venue.status_poll_ms, transfer.max_time_s (существующие).

use rust_decimal::Decimal;

use crate::storage::journal::Level;

use super::deal::Deal;
use super::deal_words as w;
use super::engine::Core;
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

/// Итог чистого решения остатка: тексты для журнала.
pub struct Resolved {
    /// Вышло меньше количества — Warn и Telegram.
    pub short: Option<String>,
    /// Взято сверх ожидаемого зачисления (остатки прошлых сделок, решение 76) — Info в журнал.
    pub over: Option<String>,
}

/// Что делает: решение остатка в тихий момент (чистая часть; вызывать под замком счёта — locked): количество =
/// min(количество + остаток, факт − подтверждённые других). Принимает: сделку, факт, учёт других, ожидаемое удержание
/// %. Возвращает: тексты для журнала (остатка нет — пусто). Ошибок нет.
pub fn resolve(deal: &mut Deal, fact: Decimal, o: &Others, hold: Decimal) -> Resolved {
    let Some(rest) = super::credit_fact::rest(deal) else {
        return Resolved {
            short: None,
            over: None,
        };
    };
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
    let mut over = None;
    if qty > expected && expected >= Decimal::ZERO {
        let n = w::coins(qty - expected, &t);
        text.push_str(&format!(
            "; взято сверх зачисления {n} — остатки прошлых сделок (решение 76)"
        ));
        over = Some(format!("{}: {text}", deal.id));
    }
    deal.note(text.clone());
    let short = (qty < before).then(|| {
        format!(
            "{}: {text}; не хватает {}",
            deal.id,
            w::coins(before - qty, &t)
        )
    });
    Resolved { short, over }
}

/// Что делает: чтение факта и решение остатка под одним замком счёта (шапка); журнал core.credit_fact — Info «взято
/// сверх зачисления», Warn и Telegram — «не хватает». Принимает: ядро, сделку. Возвращает: Res. Ошибок нет.
pub async fn locked(core: &Core, deal: &mut Deal) -> Res {
    let probe = deal.clone();
    let hold = super::deposit_hold::expected_pct(core, &deal.at, &deal.direction.asset);
    let mut out = None;
    let res = Box::pin(super::remain_read::with_fact(core, &probe, |r, o| {
        match (busy(core, deal, &o), r) {
            (Some(why), _) => Res::Busy(why),
            (None, Err(e)) => Res::Unread(e),
            (None, Ok(fact)) => {
                out = Some(resolve(deal, fact, &o, hold));
                super::credit_lock::sync(core, deal);
                Res::Done
            }
        }
    }))
    .await;
    let (deps, v) = (&core.inner.deps, deal.at.0.clone());
    let out = out.unwrap_or(Resolved {
        short: None,
        over: None,
    });
    if let Some(t) = out.over {
        Box::pin(deps.log(Level::Info, "core.credit_fact", Some(&v), &t, None)).await;
    }
    if let Some(t) = out.short {
        Box::pin(deps.log(Level::Warn, "core.credit_fact", Some(&v), &t, None)).await;
        Box::pin(deps.notify("deal.credit_hold", Some(&v), &t)).await;
    }
    res
}
