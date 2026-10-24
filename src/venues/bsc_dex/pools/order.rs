//! Файл: venues/bsc_dex/pools/order.rs
//! Что делает: порядок и отбор взятых пулов перед подпиской (решение 61). Только порядок, без отбора. Загрузка состояния пула — самое дорогое в
//! очереди узла, поэтому сначала грузятся пулы монет подписки (ядро уже отобрало прошедшие фильтр биржи: сеть BSC с
//! адресом и торгуемая спот-пара, core/subscribe.rs dex_tradable), а внутри монеты — по ликвидности индексатора
//! (reserve_in_usd) по убыванию; между монетами — по кругу: сначала самый ликвидный пул каждой монеты, потом второй
//! и т.д. Так ни одна монета не ждёт, пока загрузятся все пулы соседей.
//! ВАЖНО (решение 58 в силе): ликвидность индексатора — ТОЛЬКО очерёдность загрузки. Ни один пул из-за неё не
//! отбрасывается: слушаются все взятые; в допуске пула, котировке и расчёте она не участвует — там только состояние
//! пула со своего узла (dex.min_pool_liquidity_usd и др.).
//! Кто вызывает: pools/index.rs (taken_against, take).
//! От чего зависит: ctx.rs, directory (Snap: tokens, events), discover.rs (Found).
//! Настройки: нет.
//!
//! Поле reserve_in_usd пула — https://docs.coingecko.com/demo/reference/top-pools-contract-address (разбор —
//! directory/gt_parse.rs).

use alloy_primitives::Address;

use super::super::ctx::Ctx;
use super::super::directory::gt_parse::IndexedPool;
use super::super::directory::pool_key;
use super::discover::Found;

/// Пулы в порядке загрузки (все — отбора здесь нет). Принимает: взятые пулы. Возвращает: их же по порядку.
/// Ошибок нет.
pub fn order(ctx: &Ctx, found: Vec<Found>) -> Vec<Found> {
    let mut ranked: Vec<(usize, Found)> = ctx.index.with(|s| {
        found
            .into_iter()
            .map(|f| {
                let key = pool_key(&f.reference);
                let list = s.tokens.get(&f.coin).map(|t| t.pools.as_slice());
                let events = s.events.get(&f.coin).map(|v| v.as_slice());
                (
                    rank(&key, list.unwrap_or_default(), events.unwrap_or_default()),
                    f,
                )
            })
            .collect()
    });
    // По кругу между монетами: место у своей монеты, затем адрес монеты и встречный токен (устойчивый порядок).
    ranked.sort_by_key(|(r, f)| (*r, f.coin, counter(f)));
    ranked.into_iter().map(|(_, f)| f).collect()
}

/// Место пула у своей монеты: по ликвидности индексатора (reserve_in_usd) по убыванию, без значения — после, при
/// равенстве — в порядке ответа индексатора; пулы из событий создания — после них; не найден — последним.
/// Ликвидность индексатора здесь — только очерёдность загрузки (решение 58: в допуске и расчёте не участвует).
/// Ошибок нет.
fn rank(key: &str, list: &[IndexedPool], events: &[IndexedPool]) -> usize {
    let mut by_liq: Vec<&IndexedPool> = list.iter().collect();
    // Стабильная сортировка: при равной ликвидности — порядок индексатора.
    by_liq.sort_by(|a, b| {
        b.reserve_usd
            .is_some()
            .cmp(&a.reserve_usd.is_some())
            .then(b.reserve_usd.cmp(&a.reserve_usd))
    });
    if let Some(i) = by_liq.iter().position(|p| p.pool == key) {
        return i;
    }
    match events.iter().position(|p| p.pool == key) {
        Some(i) => list.len() + i,
        None => usize::MAX,
    }
}

/// Встречный токен пула (не монета). Ошибок нет.
fn counter(f: &Found) -> Address {
    if f.token0 == f.coin {
        f.token1
    } else {
        f.token0
    }
}
