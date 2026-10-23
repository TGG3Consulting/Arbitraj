//! Файл: venues/bsc_dex/pools/discover_v3.rs
//! Что делает: поиск пулов V3 с USDT по событиям фабрики PoolCreated (USDT — token0 или token1).
//! Уровни комиссии не зашиты: fee и tickSpacing берутся из самого события.
//! Кто вызывает: pools/discover.rs.
//! От чего зависит: ctx.rs, pools/scan.rs, pools/abi_v2v3.rs, discover.rs (Found).
//! Настройки: dex.factory [pancakeswap_v3 / uniswap_v3], dex.scan_from_block, dex.logs_block_batch.
//!
//! PoolCreated(address indexed token0, address indexed token1, uint24 indexed fee, int24 tickSpacing, address pool) —
//! https://github.com/pancakeswap/pancake-v3-contracts/blob/986847948755cba528324d41be19480731c36c2a/projects/v3-core/contracts/interfaces/IPancakeV3Factory.sol#L23-L29 ,
//! Uniswap — https://github.com/Uniswap/v3-core/blob/5ea68a77cb541c6b3812df783be71ba8c081e25f/contracts/interfaces/IUniswapV3Factory.sol#L18-L24 .
//! «читать уровни из событий фабрики, не зашивать» — research/flow/bsc_dex.md, раздел 2.

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};
use serde_json::{Value, json};

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::abi_v2v3::IV3Factory;
use super::discover::Found;
use super::families::Family;
use super::scan;

/// Все пулы V3 с USDT за блоки [from, to].
/// Ошибки: NotConfigured (dex.factory, dex.logs_block_batch), узел, BadResponse (событие не по ABI).
pub async fn scan(
    ctx: &Ctx,
    f: &'static Family,
    usdt: Address,
    from: u64,
    to: u64,
) -> GatewayResult<Vec<Found>> {
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let t0 = IV3Factory::PoolCreated::SIGNATURE_HASH;
    let u = B256::left_padding_from(usdt.as_slice());
    let mut out = Vec::new();
    for topics in [json!([t0, u]), json!([t0, Value::Null, u])] {
        for log in scan::logs(
            ctx,
            factory,
            topics,
            from,
            to,
            &format!("{}: PoolCreated", f.title),
        )
        .await?
        {
            let e = IV3Factory::PoolCreated::decode_log_data(&log.inner.data).map_err(|e| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("{}: PoolCreated не по ABI: {e}", f.title),
                )
            })?;
            let coin = if e.token0 == usdt { e.token1 } else { e.token0 };
            out.push(Found {
                family: f,
                reference: PoolRef::Contract(e.pool),
                token0: e.token0,
                token1: e.token1,
                coin,
                fee_pips: Some(e.fee.to::<u32>()),
                tick_spacing: Some(e.tickSpacing.as_i32()),
                bin_step: None,
                key: None,
                hooks: None,
            });
        }
    }
    Ok(out)
}
