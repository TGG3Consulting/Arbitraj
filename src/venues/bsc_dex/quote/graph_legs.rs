//! Файл: venues/bsc_dex/quote/graph_legs.rs
//! Что делает: превращает путь графа (quote/route_paths.rs) в ноги расчёта (quote/route_calc.rs) для стороны сделки:
//! ориентирует хопы (покупка — порядок пути USDT→монета, продажа — обратный), берёт допуск каждого пула
//! (quote/select.rs) по стороне ПУЛА (направлению свопа на этом хопе, не всей сделки) и decimals токенов из кэша.
//! Фаза 80, PR 80.4 — оценка пути и сплит (бэклог §7.3).
//! Кто вызывает: quote/graph_route.rs.
//! От чего зависит: quote/route_calc.rs (Leg), quote/route_paths.rs (RoutePath), quote/select.rs, pools/registry.rs.
//! Настройки: через select.rs — см. его шапку.

use std::collections::HashMap;

use alloy_primitives::Address;

use crate::core::types::Side;

use super::super::pools::registry::Entry;
use super::Quoting;
use super::route_calc::Leg;
use super::route_paths::RoutePath;
use super::select;

/// Сторона ПУЛА для хопа по токену входа: Buy — в пул идёт встречный токен (usdt-поле), выходит монета (coin);
/// Sell — наоборот. None — токен не является концом пула (не должно случаться). Ошибок нет.
pub fn hop_side(e: &Entry, token_in: Address) -> Option<Side> {
    if token_in == e.usdt {
        Some(Side::Buy)
    } else if token_in == e.coin {
        Some(Side::Sell)
    } else {
        None
    }
}

/// Ноги пути в порядке исполнения стороны сделки. `side` — сторона всей сделки (Buy: USDT→монета; Sell: монета→USDT).
/// Допуск пула берётся по стороне пула каждого хопа (hop_side), decimals — из `dec`.
/// Ошибка (текст) — пул не допущен или нет decimals: такой путь вызывающий пропускает.
pub fn legs_for(
    q: &Quoting,
    path: &RoutePath,
    side: Side,
    usdt_dec: u8,
    dec: &HashMap<Address, u8>,
) -> Result<Vec<Leg>, String> {
    // Хопы в порядке исполнения: покупка — как в пути, продажа — обратный с переворотом токенов хопа.
    let order: Vec<(&Entry, Address, Address)> = match side {
        Side::Buy => path
            .steps
            .iter()
            .map(|s| (&s.edge.entry, s.token_in, s.token_out))
            .collect(),
        Side::Sell => path
            .steps
            .iter()
            .rev()
            .map(|s| (&s.edge.entry, s.token_out, s.token_in))
            .collect(),
    };
    let mut legs = Vec::with_capacity(order.len());
    for (entry, token_in, token_out) in order {
        let hs = hop_side(entry, token_in).ok_or("ребро не соединяет токены хопа")?;
        let usable = select::usable(q, entry, hs, usdt_dec)
            .map_err(|w| format!("{} {}: {w}", entry.family.title, entry.ticker))?;
        let dec_in = *dec.get(&token_in).ok_or("нет decimals токена входа")?;
        let dec_out = *dec.get(&token_out).ok_or("нет decimals токена выхода")?;
        legs.push(Leg {
            entry: entry.clone(),
            usable,
            token_in,
            token_out,
            dec_in,
            dec_out,
        });
    }
    Ok(legs)
}
