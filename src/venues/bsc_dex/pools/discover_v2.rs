//! Файл: venues/bsc_dex/pools/discover_v2.rs
//! Что делает: поиск пар V2 «монета/USDT» вызовом фабрики getPair(монета, USDT) на текущем блоке.
//! Нулевой адрес — пары нет.
//! Кто вызывает: pools/discover.rs.
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_v2v3.rs, discover.rs (Found).
//! Настройки: dex.factory [pancakeswap_v2 / uniswap_v2].
//!
//! getPair: «returns the address of the pair for tokenA and tokenB, if it has been created, else address(0)»;
//! заполняется в обе стороны — https://github.com/Uniswap/v2-core/blob/6a9e7c97860676e0992f22a49665760444c1cdf5/contracts/UniswapV2Factory.sol#L23-L41
//! (PancakeSwap — то же, research/flow/bsc_dex.md, раздел 2).

use std::collections::HashMap;

use alloy_primitives::Address;

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call::{self, block_tag};
use super::abi_v2v3::IV2Factory;
use super::discover::{Found, sorted};
use super::families::Family;

/// Пары V2 монет с USDT.
/// Принимает: вид пула, монеты, USDT, блок. Возвращает: найденные пары.
/// Ошибки: NotConfigured (dex.factory), узел — по любой монете (пол-списка — не факт).
pub async fn find(
    ctx: &Ctx,
    f: &'static Family,
    coins: &HashMap<Address, String>,
    usdt: Address,
    block: u64,
) -> GatewayResult<Vec<Found>> {
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let tag = block_tag(block);
    let list: Vec<Address> = coins.keys().copied().collect();
    let calls: Vec<_> = list
        .iter()
        .map(|item| IV2Factory::getPairCall {
            tokenA: *item,
            tokenB: usdt,
        })
        .collect();
    let found = call::view_list(ctx, factory, &calls, &tag).await?;
    let mut out = Vec::new();
    for (coin, pair) in list.iter().zip(found) {
        if pair == Address::ZERO {
            continue;
        }
        let (token0, token1) = sorted(*coin, usdt);
        out.push(Found {
            family: f,
            reference: PoolRef::Contract(pair),
            token0,
            token1,
            coin: *coin,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            key: None,
            hooks: None,
        });
    }
    Ok(out)
}
