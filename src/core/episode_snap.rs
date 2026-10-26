//! Файл: core/episode_snap.rs
//! Что делает: одна запись эпизода (решение 74) — снимок рынка и пересчёт на момент записи: время, верхние
//! episode.book_levels уровней стакана каждой биржи направления (обе стороны, время биржи), состояние пулов маршрута
//! DEX с номером блока (как ядро их получило — core/types/pool.rs), пересчитанные прибыль на бюджете и карточка
//! (глубины пулов, максимальный объём и прибыль без бюджета).
//! Кто вызывает: core/episode_run.rs.
//! От чего зависит: card.rs, costs.rs, engine.rs (рынок), core/types.
//! Настройки: нет (число уровней передаёт episode_run.rs из episode.book_levels).

use chrono::Utc;
use serde_json::{Value, json};

use crate::core::types::{OrderBook, Pool, PoolRef};

use super::card::Card;
use super::costs::Calc;
use super::engine::{Core, rd};

/// Что делает: строка записи. Принимает: ядро, расчёт, карточку, число уровней стакана. Возвращает: JSON. Ошибок нет.
pub fn record(core: &Core, calc: &Calc, card: &Card, levels: usize) -> Value {
    let d = &calc.direction;
    let market = rd(&core.inner.market);
    let books: Vec<Value> = [&d.buy_on, &d.sell_on]
        .into_iter()
        .filter_map(|v| market.book(v, &d.asset.ticker))
        .map(|b| top(b, levels))
        .collect();
    let refs: Vec<&PoolRef> = [&calc.buy_quote.route, &calc.sell_quote.route]
        .into_iter()
        .flatten()
        .flat_map(|r| r.legs.iter().flat_map(|l| &l.hops))
        .map(|h| &h.pool)
        .collect();
    let pools: Vec<&Pool> = market
        .pools
        .iter()
        .filter(|((v, r), _)| (v == &d.buy_on || v == &d.sell_on) && refs.contains(&r))
        .map(|(_, p)| p)
        .collect();
    json!({
        "type": "calc",
        "t": Utc::now(),
        "qty": calc.qty,
        "profit_usd": calc.profit_usd,
        "profit_pct": calc.profit_pct,
        "buy_avg": calc.buy_avg,
        "sell_avg": calc.sell_avg,
        "books": books,
        "pools": pools,
        "card": card,
    })
}

/// Верхние уровни стакана. Ошибок нет.
fn top(b: &OrderBook, n: usize) -> Value {
    json!({
        "venue": b.venue,
        "exchange_time": b.exchange_time,
        "received_at": b.received_at,
        "bids": b.bids.iter().take(n).collect::<Vec<_>>(),
        "asks": b.asks.iter().take(n).collect::<Vec<_>>(),
    })
}
