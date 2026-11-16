//! Файл: core/feed_need.rs
//! Что делает: решение 366 (задача t163, доработка по ревью Н2) — какие пары площадки нужны торговле прямо сейчас:
//! их стакан сторож пар подтверждает у биржи заранее и первыми в ведре запросов (feed_confirm_queue.rs). Пара нужна,
//! если по направлению с этой площадкой (покупка или продажа):
//! - строка «Живого экрана» в состоянии «держится» (Holding) или «кандидат» (Candidate) — spreads;
//! - направление стоит в очереди тяжёлого расчёта или считается (queued — снимается после расчёта, candidates.rs).
//!
//! Монеты идущих сделок сторож получает отдельно (feed_hold.rs holders, аргумент deals прохода). Тикер — площадки
//! (решение 211, venue_ticker.rs), как в PairHealth.ticker.
//! Кто вызывает: core/feed_watch.rs (перед каждым проходом). От чего зависит: engine.rs (spreads, queued),
//! signal_types.rs (State), venue_ticker.rs.
//! Настройки: нет.

use std::collections::HashSet;

use crate::core::types::{Asset, Direction, VenueId};

use super::engine::{Core, lk, rd};
use super::signal_types::State;

/// Направление касается площадки (покупка или продажа на ней). Ошибок нет.
fn on(d: &Direction, v: &VenueId) -> bool {
    &d.buy_on == v || &d.sell_on == v
}

/// Тикеры площадки, нужные торговле сейчас. Принимает: ядро, площадку. Возвращает: множество тикеров площадки.
/// Ошибок нет.
pub fn needed(core: &Core, venue: &VenueId) -> HashSet<String> {
    let mut assets: Vec<Asset> = rd(&core.inner.spreads)
        .iter()
        .filter(|(d, s)| on(d, venue) && matches!(s.state, State::Holding | State::Candidate))
        .map(|(d, _)| d.asset.clone())
        .collect();
    assets.extend(
        lk(&core.inner.queued)
            .iter()
            .filter(|d| on(d, venue))
            .map(|d| d.asset.clone()),
    );
    assets
        .iter()
        .map(|a| super::venue_ticker::ticker_on(core, a, venue))
        .collect()
}
