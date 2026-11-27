//! Файл: venues/bsc_dex/pools/market_mids.rs
//! Что делает: пулы для промежуточных монет (решение 52) при подписке на рынок: «монета — промежуточная» для всех
//! монет подписки (кроме самой промежуточной) и наблюдатели новых таких пулов. Пулы «промежуточная — USDT» ищутся
//! вместе с обычными пулами с USDT (промежуточная монета добавляется к монетам — pools/market.rs).
//! Кто вызывает: pools/market.rs.
//! От чего зависит: discover.rs, created.rs (Watch), mids.rs.
//! Настройки: через вызываемые — dex.* [вид пула].
//!
//! Пулы — те же, что для USDT: из справочника индексатора, проверенные на узле (решение 58, pools/index.rs).

use std::collections::HashMap;

use alloy_primitives::Address;

use super::super::ctx::Ctx;
use super::discover::{self, Found};
use super::mids::Mid;

/// Монеты наблюдателя новых пулов и его встречный токен (промежуточная монета) — по каждой промежуточной.
pub type MidWatches = Vec<(HashMap<Address, String>, Address)>;

/// Пулы «монета — промежуточная» и монеты для наблюдателей новых пулов по каждой промежуточной.
/// Принимает: окружение, монеты подписки, промежуточные монеты.
/// Возвращает: (пулы, [(монеты, промежуточная)]). Ошибок нет: сбой вида пула — в журнал (discover.rs).
pub fn find(ctx: &Ctx, coins: &HashMap<Address, String>, mids: &[Mid]) -> (Vec<Found>, MidWatches) {
    let mut found = Vec::new();
    let mut watches = Vec::new();
    for m in mids {
        let mut list = coins.clone();
        list.remove(&m.address);
        found.extend(discover::find_all(ctx, &list, m.address));
        watches.push((list, m.address));
    }
    (found, watches)
}

/// Ethereum: для графа с тремя шагами нужны также пары промежуточных валют с
/// разрешённой обёрткой ETH. Храним единственную ориентацию (CHF как coin, WETH как quote).
pub(super) fn wrapped_bridges(ctx:&Ctx,mids:&[Mid])->(HashMap<Address,String>,Vec<Mid>){
    if ctx.chain_id()!=Some(1) || ctx.text("dex.router_mode").ok().as_deref()!=Some("graph")
        || ctx.int("dex.route_max_hops").unwrap_or(0)<3 { return Default::default(); }
    let Ok(wrapped)=ctx.addr("dex.native_wrapped") else{return Default::default();};
    let Some(w)=mids.iter().find(|m|m.address==wrapped) else{return Default::default();};
    (mids.iter().filter(|m|m.address!=wrapped).map(|m|(m.address,m.ticker.clone())).collect(),vec![w.clone()])
}

/// Монеты подписки вместе с промежуточными (для пулов «промежуточная — USDT» и проверок налогов). Ошибок нет.
pub fn with_mids(coins: &HashMap<Address, String>, mids: &[Mid]) -> HashMap<Address, String> {
    let mut all = coins.clone();
    for m in mids {
        all.entry(m.address).or_insert_with(|| m.ticker.clone());
    }
    all
}
