//! Файл: core/sell_turn_fresh.rs
//! Что делает: решение 415 (t200) — котировка продажи сделки под ходом очереди продажи счёта (sell_turn.rs): данные
//! котировки не раньше прошлой исполненной продажи на этом ключе (след Last). Тот же признак свежести, что у следа
//! продаж групп докупки (ride_trail.rs check) и у покупки докупки (ride_quote.rs buy_fresh):
//! - биржа — стакан котировки новее конца прошлой продажи (Quote.at > Last.at);
//! - DEX — состояние пулов после блока прошлой продажи: (а) все пулы её маршрута в памяти ядра уже с блоком не раньше
//!   её блока (адаптер шлёт пул ядру ПОСЛЕ записи в свой реестр — venues/bsc_dex/pools/emit_pool.rs, поэтому
//!   проверяется ДО котировки) — или (б) маршрут котировки через её пулы (PoolRef::site) по блоку не раньше её блока
//!   (DexRoute.block — наименьший блок пулов маршрута, quote/costs.rs); маршрут котировки не через её пулы — свежо;
//!   блок её продажи неизвестен или котировка без маршрута (агрегатор) — «не доказуемо».
//!
//! Не свежо — ждём тика монеты (stuck_lag.rs Ticks: событие пула / стакана монеты) и котируем снова, не дольше
//! feed.stale_after_ms площадки (данные, не отражающие нашу же продажу дольше этого, — устаревшие). Срок вышел (или
//! настройка не задана) — продажа идёт по последней котировке, как до решения 415 (адаптер DEX всё равно сверяет цену
//! по живому состоянию перед отправкой — exec/order.rs check_price), строка Warn журнала core.sell_turn.
//! Ошибка котировки возвращается сразу (след остаётся для следующей котировки).
//! Кто вызывает: core/sell.rs, core/sell_range_run.rs (quote — под ходом до ордера); core/stuck.rs, core/relay.rs,
//! core/relay_here.rs (once — котировка-решение: ход только на время котировки).
//! От чего зависит: sell_turn.rs, trade_ops.rs (deal_quote, kind), ride_trail.rs (sites), stuck_lag.rs (Ticks),
//! market_state.rs (пулы ядра), exec_book.rs (Book), deps.rs (журнал), settings.
//! Настройки: feed.stale_after_ms [площадка] — предел ожидания данных после прошлой продажи (существующая настройка).

use std::future::Future;
use std::time::Duration;

use rust_decimal::Decimal;
use tokio::time::Instant;

use crate::core::types::{PoolRef, Quote, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::{Core, rd};
use super::op_error::OpError;
use super::sell_turn::{Last, Turn};
use super::stuck_lag::Ticks;

/// Итог котировки продажи сделки: (выручка чистыми, котировка, стакан котировки биржи) — как trade_ops::deal_quote.
pub type Quoted = Result<(Decimal, Quote, Option<super::exec_book::Book>), OpError>;

/// Что делает: котировка продажи сделки (trade_ops::deal_quote) под ходом — не раньше прошлой продажи ключа (шапка).
/// Принимает: ядро, сделку, ход. Возвращает и ошибки: как deal_quote.
pub async fn quote(core: &Core, deal: &Deal, turn: &mut Turn) -> Quoted {
    let f = || Box::pin(super::trade_ops::deal_quote(core, deal));
    Box::pin(quote_with(core, deal, turn, f)).await
}

/// Что делает: котировка-решение (режим застрявшей монеты, перегон, продажа здесь): ход берётся только на время
/// котировки и сразу снимается — котировка не попадает между котировкой и ордером другой продажи ключа. Принимает:
/// ядро, сделку. Возвращает и ошибки: как deal_quote.
pub async fn once(core: &Core, deal: &Deal) -> Quoted {
    let mut t = super::sell_turn::take(&deal.at, &deal.direction.asset).await;
    let q = Box::pin(quote(core, deal, &mut t)).await;
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
/// вызывающего). Принимает: ядро, сделку (площадка, монета тика), ход, котировку. Возвращает: котировку — свежую, или
/// последнюю по сроку; ошибку котировки — сразу. Ошибок своих нет.
pub async fn quote_with<F, Fut>(core: &Core, deal: &Deal, turn: &mut Turn, mut f: F) -> Quoted
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
            turn.seen();
            Box::pin(warn(core, deal, &last, wait)).await;
            return q;
        }
    }
}

/// Что делает: DEX — все пулы маршрута прошлой продажи в памяти ядра уже с блоком не раньше её блока (признак (а)
/// шапки). Принимает: ядро, площадку, след. Возвращает: да — состояние после неё; нет — не доказано (блок или пулы
/// неизвестны, пула нет в памяти, блок раньше). Ошибок нет.
pub fn pools_fresh(core: &Core, v: &VenueId, last: &Last) -> bool {
    let Some(b) = last.block else {
        return false;
    };
    if last.pools.is_empty() {
        return false;
    }
    let m = rd(&core.inner.market);
    last.pools.iter().all(|p| {
        m.pools
            .get(&(v.clone(), p.clone()))
            .is_some_and(|x| x.block >= b)
    })
}

/// Что делает: котировка `q` новее прошлой продажи (шапка). Принимает: ядро, площадку, след, признак (а), снятый до
/// котировки, котировку. Возвращает: да — свежая. Ошибок нет (площадки нет в реестре — не доказано).
pub fn after(core: &Core, v: &VenueId, last: &Last, early: bool, q: &Quote) -> bool {
    match super::trade_ops::kind(core, v) {
        Ok(VenueKind::Cex) => q.at > last.at,
        Ok(VenueKind::Dex) => {
            let Some(b) = last.block else {
                return false;
            };
            if early {
                return true;
            }
            let Some(r) = &q.route else {
                return false;
            };
            let mine = super::ride_trail::sites(r);
            let theirs: Vec<PoolRef> = last.pools.iter().map(PoolRef::site).collect();
            let touches = theirs.is_empty() || mine.iter().any(|p| theirs.contains(p));
            !touches || r.block >= b
        }
        Err(_) => false,
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
