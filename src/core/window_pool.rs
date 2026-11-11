//! Файл: core/window_pool.rs
//! Что делает: решение 255 (Тигран 11.11) — состояние пулов маршрута в точке цены DEX окна (решение 241, window_dex.rs;
//! решение 245 — и точка на объём сделки, window_deal_run.rs). Доказано по коду, что ликвидность пулов за всё окно нигде не
//! хранится: точка окна несёт только цену, объём, блок и адреса пулов (window_dex.rs DexPoint); состояние пулов
//! (резервы / sqrtPrice / ликвидность, core/types/pool.rs) пишут только записи эпизода (episode_snap.rs) — и только пока
//! направление проходит расчёт (episode.rs close на отказе), а окно живёт и дальше, до конца нашей сделки (window.rs
//! closable); события пулов (Swap / Sync) — в сырой журнал узла, только при raw.enabled = да и подписке WebSocket
//! (venues/bsc_dex/node/ws_task.rs), квитанции блоков по HTTP (решение 68) туда не пишутся. Поэтому к точке — копия
//! верхнего состояния пулов маршрута из памяти рынка ядра (то же, что у эпизода: Pool с номером блока и временем
//! получения), при window.dex_pool_state = да. Новых запросов к узлу нет; копирование — под коротким чтением рынка.
//! Кто вызывает: core/window_dex.rs (note), core/window_deal_run.rs (write).
//! От чего зависит: engine.rs (рынок), core/types (Pool, PoolRef, Quote), settings.
//! Настройки: window.dex_pool_state (не задана или «нет» — состояния не пишутся, как раньше).

use crate::core::types::{Pool, PoolRef, Quote};
use crate::settings::read::At;

use super::engine::{Core, rd};

/// Что делает: состояния пулов маршрута котировки из памяти рынка, если window.dex_pool_state = да.
/// Принимает: ядро, котировку стороны DEX. Возвращает: состояния по порядку пулов маршрута (пул без состояния в памяти
/// пропускается — его нет в рынке), каждое — Pool в JSON; пусто — настройка не включена или маршрута нет. Ошибок нет.
pub fn states(core: &Core, q: &Quote) -> Vec<serde_json::Value> {
    let on = core
        .inner
        .deps
        .settings
        .flag("window.dex_pool_state", At::default());
    if !matches!(on, Ok(true)) {
        return vec![];
    }
    let mut refs: Vec<&PoolRef> = vec![];
    for h in q.route.iter().flat_map(|r| &r.legs).flat_map(|l| &l.hops) {
        if !refs.contains(&&h.pool) {
            refs.push(&h.pool);
        }
    }
    if refs.is_empty() {
        return vec![];
    }
    // Под чтением рынка — только копии; в JSON — уже без замка.
    let copies: Vec<Pool> = {
        let market = rd(&core.inner.market);
        refs.into_iter()
            .filter_map(|r| market.pools.get(&(q.venue.clone(), r.clone())).cloned())
            .collect()
    };
    copies
        .iter()
        .filter_map(|p| serde_json::to_value(p).ok())
        .collect()
}
