//! Файл: core/episode_snap.rs
//! Что делает: снимок рынка для записи эпизода (решение 74). capture — под коротким чтением состояния рынка только
//! копирует нужное: верхние episode.book_levels уровней стакана каждой биржи направления (обе стороны, время биржи) и
//! верхнее состояние пулов маршрута DEX с номером блока (core/types/pool.rs: резервы / slot0 / ликвидность — карт тиков
//! у ядра нет, их держит адаптер, поэтому в кадр они и не попадают). record — сериализация уже без блокировки, в
//! задаче эпизодов: снимок + пересчёт (прибыль, цены) + карточка (глубина пулов по сторонам, максимальный объём).
//! Кто вызывает: core/episode_run.rs (capture — при открытии и записи), core/episode_worker.rs (record).
//! От чего зависит: card.rs, costs.rs, engine.rs (рынок), core/types.
//! Настройки: нет (число уровней передаёт episode_run.rs из episode.book_levels).

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::core::types::{OrderBook, Pool, PoolRef};

use super::card::Card;
use super::costs::Calc;
use super::engine::{Core, rd};

/// Копия рынка на момент пересчёта.
#[derive(Debug, Clone)]
pub struct Snap {
    pub at: DateTime<Utc>,
    pub books: Vec<OrderBook>,
    pub pools: Vec<Pool>,
}

/// Что делает: копирует нужное из состояния рынка (блокировка — только на копирование). Принимает: ядро, расчёт, число
/// уровней. Возвращает: Snap. Ошибок нет.
pub fn capture(core: &Core, calc: &Calc, levels: usize) -> Snap {
    let d = &calc.direction;
    let refs: Vec<&PoolRef> = [&calc.buy_quote.route, &calc.sell_quote.route]
        .into_iter()
        .flatten()
        .flat_map(|r| r.legs.iter().flat_map(|l| &l.hops))
        .map(|h| &h.pool)
        .collect();
    let market = rd(&core.inner.market);
    let books = [&d.buy_on, &d.sell_on]
        .into_iter()
        .filter_map(|v| market.book(v, &d.asset.ticker))
        .map(|b| OrderBook {
            bids: b.bids.iter().take(levels).cloned().collect(),
            asks: b.asks.iter().take(levels).cloned().collect(),
            ..b.clone_head()
        })
        .collect();
    let pools = market
        .pools
        .iter()
        .filter(|((v, r), _)| (v == &d.buy_on || v == &d.sell_on) && refs.contains(&r))
        .map(|(_, p)| p.clone())
        .collect();
    Snap {
        at: Utc::now(),
        books,
        pools,
    }
}

/// Что делает: строка записи эпизода. Принимает: снимок, расчёт, карточку (None — не посчитана). Возвращает: JSON.
/// Ошибок нет.
pub fn record(s: &Snap, calc: &Calc, card: Option<&Card>) -> Value {
    json!({
        "type": "calc",
        "t": s.at,
        "qty": calc.qty,
        "profit_usd": calc.profit_usd,
        "profit_pct": calc.profit_pct,
        "buy_avg": calc.buy_avg,
        "sell_avg": calc.sell_avg,
        "books": s.books,
        "pools": s.pools,
        "card": card,
    })
}

/// Заголовок стакана без уровней (уровни копируются отдельно — только верхние).
trait Head {
    fn clone_head(&self) -> OrderBook;
}

impl Head for OrderBook {
    /// Копия без уровней. Ошибок нет.
    fn clone_head(&self) -> OrderBook {
        OrderBook {
            venue: self.venue.clone(),
            asset: self.asset.clone(),
            bids: vec![],
            asks: vec![],
            exchange_time: self.exchange_time,
            received_at: self.received_at,
        }
    }
}
