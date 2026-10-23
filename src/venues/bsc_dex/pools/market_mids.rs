//! Файл: venues/bsc_dex/pools/market_mids.rs
//! Что делает: пулы для промежуточных монет (решение 52) при подписке на рынок: «монета — промежуточная» для всех
//! монет подписки (кроме самой промежуточной) и наблюдатели новых таких пулов. Пулы «промежуточная — USDT» ищутся
//! вместе с обычными пулами с USDT (промежуточная монета добавляется к монетам — pools/market.rs).
//! Кто вызывает: pools/market.rs.
//! От чего зависит: discover.rs, created.rs (Watch), mids.rs.
//! Настройки: через вызываемые — dex.* [вид пула].
//!
//! Поиск — тот же, что для USDT (события создания с адресом встречного токена в индексированном поле):
//! research/flow/bsc_dex.md, раздел 2; research/flow/bsc_v4.md, раздел 3.

use std::collections::HashMap;

use alloy_primitives::Address;

use super::super::ctx::Ctx;
use super::discover::{self, Found, Scanned};
use super::mids::Mid;

/// Пулы «монета — промежуточная» и монеты для наблюдателей новых пулов по каждой промежуточной.
/// Принимает: окружение, монеты подписки, промежуточные монеты, текущий блок, память истории.
/// Возвращает: (пулы, [(монеты, промежуточная)]). Ошибок нет: сбой вида пула — в журнал (discover.rs).
pub async fn find(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    mids: &[Mid],
    head: u64,
    scanned: &Scanned,
) -> (Vec<Found>, Vec<(HashMap<Address, String>, Address)>) {
    let mut found = Vec::new();
    let mut watches = Vec::new();
    for m in mids {
        let mut list = coins.clone();
        list.remove(&m.address);
        found.extend(discover::find_all(ctx, &list, m.address, head, scanned).await);
        watches.push((list, m.address));
    }
    (found, watches)
}

/// Монеты подписки вместе с промежуточными (для пулов «промежуточная — USDT» и проверок налогов). Ошибок нет.
pub fn with_mids(coins: &HashMap<Address, String>, mids: &[Mid]) -> HashMap<Address, String> {
    let mut all = coins.clone();
    for m in mids {
        all.entry(m.address).or_insert_with(|| m.ticker.clone());
    }
    all
}
