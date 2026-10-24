//! Файл: venues/bsc_dex/pools/discover_v3.rs
//! Что делает: поиск пулов V3 «монета — встречный токен» прямыми вызовами фабрики, без чтения истории событий:
//! уровни комиссии — из настройки dex.v3_fee_tiers (фабрика не даёт их перечня), каждый уровень сверяется с
//! фабрикой (feeAmountTickSpacing ≠ 0 — включён, оттуда же tickSpacing), затем getPool(монета, токен, fee)
//! по каждой монете и уровню. Нулевой адрес — пула нет.
//! Кто вызывает: pools/discover.rs.
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_v2v3.rs, discover.rs (Found, sorted).
//! Настройки: dex.factory, dex.v3_fee_tiers [pancakeswap_v3 / uniswap_v3].
//!
//! Официальный контракт фабрики: `mapping(uint24 => int24) public feeAmountTickSpacing` и
//! `mapping(address => mapping(address => mapping(uint24 => address))) public getPool` — перечня уровней нет
//! (только события FeeAmountEnabled):
//! https://github.com/pancakeswap/pancake-v3-contracts/blob/986847948755cba528324d41be19480731c36c2a/projects/v3-core/contracts/PancakeV3Factory.sol ,
//! https://github.com/Uniswap/v3-core/blob/5ea68a77cb541c6b3812df783be71ba8c081e25f/contracts/UniswapV3Factory.sol .
//! getPool: «tokenA and tokenB may be passed in either token0/token1 or token1/token0 order», 0 — пула нет —
//! IPancakeV3Factory.sol#L60-L70 (research/flow/bsc_dex.md, раздел 2).

use std::collections::HashMap;

use alloy_primitives::Address;
use alloy_primitives::aliases::U24;
use futures_util::future::join_all;

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call::{self, block_tag};
use super::abi_v2v3::IV3Factory;
use super::discover::{Found, sorted};
use super::families::Family;

/// Пулы V3 монет со встречным токеном.
/// Принимает: вид пула, монеты, встречный токен (USDT или промежуточная), блок.
/// Возвращает: найденные пулы. Ошибки: NotConfigured (dex.factory, dex.v3_fee_tiers, уровень не включён в фабрике),
/// узел — по любому вызову (пол-списка — не факт).
pub async fn find(
    ctx: &Ctx,
    f: &'static Family,
    coins: &HashMap<Address, String>,
    quote: Address,
    block: u64,
) -> GatewayResult<Vec<Found>> {
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let tag = block_tag(block);
    let tiers = tiers(ctx, f, factory, &tag).await?;
    let list: Vec<(Address, u32, i32)> = coins
        .keys()
        .filter(|c| **c != quote)
        .flat_map(|c| tiers.iter().map(move |(fee, sp)| (*c, *fee, *sp)))
        .collect();
    let calls = list.iter().map(|(coin, fee, _)| {
        let c = IV3Factory::getPoolCall {
            tokenA: *coin,
            tokenB: quote,
            fee: U24::from(*fee),
        };
        let tag = tag.clone();
        async move { call::view(ctx, factory, &c, &tag).await }
    });
    let mut out = Vec::new();
    for ((coin, fee, spacing), pool) in list.iter().zip(join_all(calls).await) {
        let pool = pool?;
        if pool == Address::ZERO {
            continue;
        }
        let (token0, token1) = sorted(*coin, quote);
        out.push(Found {
            family: f,
            reference: PoolRef::Contract(pool),
            token0,
            token1,
            coin: *coin,
            fee_pips: Some(*fee),
            tick_spacing: Some(*spacing),
            bin_step: None,
            key: None,
            hooks: None,
        });
    }
    Ok(out)
}

/// Уровни комиссии из dex.v3_fee_tiers, сверенные с фабрикой: (fee в миллионных долях, tickSpacing).
/// Ошибки: NotConfigured — настройка не задана, пустая, не число uint24 или уровень не включён в фабрике
/// (feeAmountTickSpacing = 0); узел.
async fn tiers(
    ctx: &Ctx,
    f: &Family,
    factory: Address,
    tag: &str,
) -> GatewayResult<Vec<(u32, i32)>> {
    let fees = parse_tiers(&ctx.dex_list("dex.v3_fee_tiers", f.id)?)
        .map_err(|m| ctx.err(ErrorCode::NotConfigured, format!("{}: {m}", f.title)))?;
    let mut out = Vec::with_capacity(fees.len());
    for fee in fees {
        let c = IV3Factory::feeAmountTickSpacingCall {
            fee: U24::from(fee),
        };
        let spacing = call::view(ctx, factory, &c, tag).await?.as_i32();
        if spacing == 0 {
            let m = format!(
                "{}: уровень {fee} из dex.v3_fee_tiers не включён в фабрике (feeAmountTickSpacing = 0)",
                f.title
            );
            return Err(ctx.err(ErrorCode::NotConfigured, m));
        }
        out.push((fee, spacing));
    }
    Ok(out)
}

/// Разбор списка уровней: целые uint24 (100 = 0,01%). Ошибка — текст причины (пусто или не число).
pub fn parse_tiers(list: &[String]) -> Result<Vec<u32>, String> {
    if list.is_empty() {
        return Err("dex.v3_fee_tiers пуст — задайте уровни комиссии".into());
    }
    list.iter()
        .map(|s| match s.trim().parse::<u32>() {
            Ok(v) if v < (1 << 24) => Ok(v),
            _ => Err(format!(
                "dex.v3_fee_tiers: «{s}» — не уровень комиссии (целое uint24, 100 = 0,01%)"
            )),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_tiers;

    #[test]
    fn tiers_parse_and_reject() {
        let ok: Vec<String> = ["100", " 2500"].iter().map(|s| s.to_string()).collect();
        assert_eq!(parse_tiers(&ok).unwrap(), vec![100, 2500]);
        assert!(parse_tiers(&[]).is_err());
        assert!(parse_tiers(&["0,25%".to_string()]).is_err());
        assert!(parse_tiers(&["16777216".to_string()]).is_err());
    }
}
