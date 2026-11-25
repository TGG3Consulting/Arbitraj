//! Файл: core/sell_turn_fresh.rs
//! Что делает: решение 415 (t200) — котировка продажи сделки под ходом очереди продажи счёта (sell_turn.rs): данные
//! котировки не раньше прошлой исполненной продажи на этом ключе (след Last; признаки свежести — sell_turn_last.rs).
//!
//! Не свежо — ждём тика монеты (stuck_lag.rs Ticks: событие пула / стакана монеты) и котируем снова, не дольше
//! feed.stale_after_ms площадки (данные, не отражающие нашу же продажу дольше этого, — устаревшие). Срок вышел (или
//! настройка не задана) — продажа идёт по последней котировке, как до решения 415 (площадка всё равно сверяет цену с
//! худшей ценой запроса перед отправкой ордера), строка Warn журнала core.sell_turn.
//! Ошибка котировки возвращается сразу (след остаётся для следующей котировки).
//! Кто вызывает: core/sell.rs, core/sell_range_run.rs (quote — под ходом до ордера); core/stuck.rs, core/relay.rs,
//! core/relay_here.rs (once — котировка-решение: ход только на время котировки).
//! Ревью 2 t200 (М-1): котировка-решение (once, Use::Decision) свежести не ждёт — одна котировка под ходом, без
//! ожидания и строки Warn, след не трогает; ждёт свежести и снимает след только котировка продажи (Use::Sale).
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
    /// Котировка-решение (ревью 2, М-1): одна котировка под ходом — без ожидания свежести и Warn, след не трогает.
    Decision,
}

/// Итог котировки продажи сделки: (выручка чистыми, котировка, стакан котировки биржи) — как trade_ops::deal_quote.
pub type Quoted = Result<(Decimal, Quote, Option<super::exec_book::Book>), OpError>;

/// Что делает: котировка продажи сделки (trade_ops::deal_quote) под ходом — не раньше прошлой продажи ключа (шапка).
/// Принимает: ядро, сделку, ход. Возвращает и ошибки: как deal_quote.
pub async fn quote(core: &Core, deal: &Deal, turn: &mut Turn) -> Quoted {
    Box::pin(quote_as(core, deal, turn, Use::Sale)).await
}

/// Apply the return fee to automatic guards only. Manual, unconditional sale can
/// still execute when the withdrawal directory is temporarily unavailable.
pub(super) async fn settled(core: &Core, deal: &Deal, quote: Quoted) -> Quoted {
    let (net, q, book) = quote?;
    if !super::manual_price::needs_individual_return(core, deal) {
        return Ok((net, q, book));
    }
    let cash = super::manual_price::sale_cash(core, &deal.at, deal.qty * q.avg_price, &q.costs)?;
    let net = super::manual_price::individual_value(core, deal, &deal.at, net, cash).await?;
    Ok((net, q, book))
}

pub(super) async fn guarded(core: &Core, deal: &Deal, turn: &mut Turn) -> Quoted {
    let raw = Box::pin(quote(core, deal, turn)).await;
    Box::pin(settled(core, deal, raw)).await
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
    let raw = Box::pin(once(core, deal)).await;
    Box::pin(settled(core, deal, raw))
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
    // Ревью 2 t200 (М-1): решению свежесть не нужна — его продажа проверит её сама.
    let Some(last) = turn.last().cloned().filter(|_| u == Use::Sale) else {
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
            turn.seen();
            Box::pin(warn(core, deal, &last, wait)).await;
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
/// срок (None — настройка не задана). Ошибок нет.
async fn warn(core: &Core, deal: &Deal, last: &Last, wait: Option<u64>) {
    let why = match wait {
        Some(ms) => format!("за feed.stale_after_ms = {ms} мс данных после неё не пришло"),
        None => "feed.stale_after_ms площадки не задана — ждать нечем".to_string(),
    };
    let text = format!(
        "{}: котировка продажи {} на {} не доказуемо новее продажи сделки {} ({}{}) — {why}; продажа идёт по \
         последней котировке, как до решения 415",
        deal.id,
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
