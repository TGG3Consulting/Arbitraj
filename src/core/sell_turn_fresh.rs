//! Файл: core/sell_turn_fresh.rs
//! Что делает: решение 415 (t200) — котировка продажи сделки под ходом очереди продажи счёта (sell_turn.rs): данные
//! котировки не раньше прошлой исполненной продажи на этом ключе (след Last; признаки свежести — sell_turn_last.rs).
//!
//! Не свежо — ждём тика монеты (stuck_lag.rs Ticks: событие пула / стакана монеты) и котируем снова, не дольше
//! feed.stale_after_ms площадки (данные, не отражающие нашу же продажу дольше этого, — устаревшие). Срок вышел (или
//! настройка не задана) — продажа идёт по последней котировке, как до решения 415 (адаптер DEX всё равно сверяет цену
//! по живому состоянию перед отправкой — exec/order.rs check_price), строка Warn журнала core.sell_turn.
//! Ошибка котировки возвращается сразу (след остаётся для следующей котировки).
//! Кто вызывает: core/sell.rs, core/sell_range_run.rs (quote — под ходом до ордера); core/stuck.rs, core/relay.rs,
//! core/relay_here.rs (once — котировка-решение: ход только на время котировки).
//! Ревью t200 (М-1): котировка-решение (once, Use::Decision) по сроку след не снимает — его проверит продажа; свой
//! текст Warn.
//! От чего зависит: sell_turn.rs, sell_turn_last.rs (pools_fresh, after), trade_ops.rs (deal_quote), stuck_lag.rs
//! (Ticks), exec_book.rs (Book), deps.rs (журнал), settings.
//! Настройки: feed.stale_after_ms [площадка] — предел ожидания данных после прошлой продажи (существующая настройка).

use std::future::Future;
use std::time::Duration;

use rust_decimal::Decimal;
use tokio::time::Instant;

use crate::core::types::{Quote, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;
use super::sell_turn::{Last, Turn};
use super::sell_turn_last::{after, pools_fresh};
use super::stuck_lag::Ticks;

/// Для чего котировка: продажа (котировка ведёт к ордеру под тем же ходом) или решение (ход — только на котировку).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    /// Котировка продажи: срок вышел — продажа по последней котировке, след снят.
    Sale,
    /// Котировка-решение (М-1): срок вышел — решение по последней котировке, след НЕ снят (его проверит продажа).
    Decision,
}

/// Итог котировки продажи сделки: (выручка чистыми, котировка, стакан котировки биржи) — как trade_ops::deal_quote.
pub type Quoted = Result<(Decimal, Quote, Option<super::exec_book::Book>), OpError>;

/// Что делает: котировка продажи сделки (trade_ops::deal_quote) под ходом — не раньше прошлой продажи ключа (шапка).
/// Принимает: ядро, сделку, ход. Возвращает и ошибки: как deal_quote.
pub async fn quote(core: &Core, deal: &Deal, turn: &mut Turn) -> Quoted {
    Box::pin(quote_as(core, deal, turn, Use::Sale)).await
}

/// Что делает: как quote, для продажи или решения (Use). Принимает: ядро, сделку, ход, назначение. Возвращает и
/// ошибки: как deal_quote.
pub async fn quote_as(core: &Core, deal: &Deal, turn: &mut Turn, u: Use) -> Quoted {
    let f = || Box::pin(super::trade_ops::deal_quote(core, deal));
    Box::pin(quote_with(core, deal, turn, u, f)).await
}

/// Что делает: котировка-решение (режим застрявшей монеты, перегон, продажа здесь): ход берётся только на время
/// котировки и сразу снимается — котировка не попадает между котировкой и ордером другой продажи ключа. Принимает:
/// ядро, сделку. Возвращает и ошибки: как deal_quote.
pub async fn once(core: &Core, deal: &Deal) -> Quoted {
    let mut t = super::sell_turn::take(&deal.at, &deal.direction.asset).await;
    let q = Box::pin(quote_as(core, deal, &mut t, Use::Decision)).await;
    drop(t);
    q
}

/// Что делает: как once, в форме trade_ops::deal_value (выручка и котировка, ошибка — текстом). Принимает: ядро,
/// сделку. Ошибки: текст причины.
pub async fn value(core: &Core, deal: &Deal) -> Result<(Decimal, Quote), String> {
    Box::pin(once(core, deal))
        .await
        .map(|(v, q, _)| (v, q))
        .map_err(|e| e.message)
}

/// Что делает: котировка `f` под ходом с проверкой свежести и ожиданием (шапка). Открыта для тестов (котировка — от
/// вызывающего). Принимает: ядро, сделку (площадка, монета тика), ход, назначение (Use), котировку. Возвращает:
/// котировку — свежую, или последнюю по сроку; ошибку котировки — сразу. Ошибок своих нет.
pub async fn quote_with<F, Fut>(
    core: &Core,
    deal: &Deal,
    turn: &mut Turn,
    u: Use,
    mut f: F,
) -> Quoted
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Quoted>,
{
    let Some(last) = turn.last().cloned() else {
        return f().await;
    };
    let v = &deal.at;
    let wait = stale_ms(core, v);
    let until = Instant::now() + Duration::from_millis(wait.unwrap_or(0));
    // Подписка — до первой котировки: тик, пришедший во время котировки, не теряется (как в stuck.rs).
    let mut ticks = Ticks::new(core);
    loop {
        let early = pools_fresh(core, v, &last);
        let q = f().await;
        let fresh = match &q {
            Ok((_, x, _)) => after(core, v, &last, early, x),
            Err(_) => return q,
        };
        if fresh {
            turn.seen();
            return q;
        }
        if Instant::now() >= until || !ticks.next(core, deal, Some(until)).await {
            // М-1: решение след не снимает — его проверит котировка самой продажи.
            if u == Use::Sale {
                turn.seen();
            }
            Box::pin(warn(core, deal, &last, wait, u)).await;
            return q;
        }
    }
}

/// feed.stale_after_ms площадки (None — не задана). Ошибок нет.
fn stale_ms(core: &Core, v: &VenueId) -> Option<u64> {
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    core.inner
        .deps
        .settings
        .integer("feed.stale_after_ms", at)
        .ok()
}

/// Строка Warn журнала core.sell_turn: данные после прошлой продажи не пришли за срок. Принимает: ядро, сделку, след,
/// срок (None — настройка не задана), назначение котировки (М-1: у решения свой текст). Ошибок нет.
async fn warn(core: &Core, deal: &Deal, last: &Last, wait: Option<u64>, u: Use) {
    let why = match wait {
        Some(ms) => format!("за feed.stale_after_ms = {ms} мс данных после неё не пришло"),
        None => "feed.stale_after_ms площадки не задана — ждать нечем".to_string(),
    };
    let then = match u {
        Use::Sale => "продажа идёт по последней котировке, как до решения 415",
        Use::Decision => "решение по последней котировке (продажа проверит свежесть заново)",
    };
    let text = format!(
        "{}: котировка {} {} на {} не доказуемо новее продажи сделки {} ({}{}) — {why}; {then}",
        deal.id,
        match u {
            Use::Sale => "продажи",
            Use::Decision => "для решения",
        },
        deal.direction.asset.ticker,
        deal.at.0,
        last.deal,
        last.at,
        last.block
            .map(|b| format!(", блок {b}"))
            .unwrap_or_default()
    );
    core.inner
        .deps
        .log(Level::Warn, "core.sell_turn", Some(&deal.at.0), text, None)
        .await;
}
