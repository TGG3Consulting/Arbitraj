//! Файл: venues/bsc_dex/quote/graph_legs.rs
//! Что делает: превращает путь графа (quote/route_paths.rs) в ноги расчёта (quote/route_calc.rs) для стороны сделки:
//! ориентирует хопы (покупка — порядок пути USDT→монета, продажа — обратный), берёт допуск каждого пула
//! (quote/select.rs) по стороне ПУЛА (направлению свопа на этом хопе, не всей сделки) и decimals токенов из кэша.
//! Фаза 80, PR 80.4 — оценка пути и сплит (бэклог §7.3).
//! decimals всех токенов путей (decimals — вынесено из quote/graph_route.rs).
//! Решение 311 (А): legs_prep — допуск всех хопов с одной подготовкой глубины (Prep) на котировку; (В) — путь во
//! владение, запись пула переходит в ногу без копии. prep_once — выключатель (А) dex.quote_prep_once (не задана — включено).
//! Кто вызывает: quote/graph_route.rs.
//! От чего зависит: quote/route_calc.rs (Leg), quote/route_paths.rs (RoutePath), quote/select.rs, pools/registry.rs,
//! pools/depth_key.rs (Prep).
//! Настройки: dex.quote_prep_once [bsc_dex] (решение 311 А, prep_once); через select.rs — см. его шапку.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use alloy_primitives::Address;

use crate::core::types::Side;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::pools::depth_key::Prep;
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

/// Решение 311 А, выключатель (Тигран 16.11.2026: «сделай вкл выкл. по дефолту вкл»): одна подготовка глубины на
/// котировку графа включена? Принимает: окружение площадки. Возвращает: dex.quote_prep_once = нет — false (подготовка
/// на каждый хоп, как до 311 А); да, не задана или ошибка чтения — true (по умолчанию включено). Ошибок нет.
pub fn prep_once(ctx: &Ctx) -> bool {
    ctx.flag("dex.quote_prep_once").unwrap_or(true)
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
    legs_prep(q, path.clone(), side, usdt_dec, dec, None)
}

/// Решение 311 (А): то же, что legs_for, с одной подготовкой глубины (`prep`, pools/depth_key.rs Prep) на все хопы —
/// её создаёт котировка графа один раз на котировку (quote/graph_route.rs). None — подготовка на каждый хоп, как
/// legs_for. Решение 311 (В): путь принимается во владение — запись пула переходит в ногу без копии (legs_for копирует
/// путь целиком — одна копия записи на хоп, как раньше). Принимает, возвращает, ошибки: как legs_for.
pub fn legs_prep(
    q: &Quoting,
    path: RoutePath,
    side: Side,
    usdt_dec: u8,
    dec: &HashMap<Address, u8>,
    prep: Option<&Prep>,
) -> Result<Vec<Leg>, String> {
    // Хопы в порядке исполнения: покупка — как в пути, продажа — обратный с переворотом токенов хопа.
    let order: Vec<(Entry, Address, Address)> = match side {
        Side::Buy => path
            .steps
            .into_iter()
            .map(|s| (Arc::unwrap_or_clone(s.edge.entry), s.token_in, s.token_out))
            .collect(),
        Side::Sell => path
            .steps
            .into_iter()
            .rev()
            .map(|s| (Arc::unwrap_or_clone(s.edge.entry), s.token_out, s.token_in))
            .collect(),
    };
    let mut legs = Vec::with_capacity(order.len());
    for (entry, token_in, token_out) in order {
        let hs = hop_side(&entry, token_in).ok_or("ребро не соединяет токены хопа")?;
        let usable = select::usable_with(q, &entry, hs, usdt_dec, prep)
            .map_err(|w| format!("{} {}: {w}", entry.family.title, entry.ticker))?;
        let dec_in = *dec.get(&token_in).ok_or("нет decimals токена входа")?;
        let dec_out = *dec.get(&token_out).ok_or("нет decimals токена выхода")?;
        legs.push(Leg {
            entry,
            usable,
            token_in,
            token_out,
            dec_in,
            dec_out,
        });
    }
    Ok(legs)
}

/// decimals всех токенов путей — из кэша или узла один раз (вынесено из quote/graph_route.rs). Ошибки: узел (decimals).
pub async fn decimals(
    v: &super::via::Via<'_>,
    paths: &[RoutePath],
) -> GatewayResult<HashMap<Address, u8>> {
    let mut set: HashSet<Address> = HashSet::new();
    for p in paths {
        for s in &p.steps {
            set.insert(s.token_in);
            set.insert(s.token_out);
        }
    }
    let mut m = HashMap::new();
    for t in set {
        m.insert(t, v.q.tokens.decimals(&v.q.ctx, t).await?);
    }
    Ok(m)
}
