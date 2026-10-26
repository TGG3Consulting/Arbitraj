//! Файл: tests/depth_common/cl.rs
//! Что делает: пул CL (Infinity CL, Uniswap V4) из живой записи глубины 2026-10-25 — запись реестра бота: ключ пула
//! (валюты, tickSpacing), состояние (getSlot0, getLiquidity), установленные тики (getPoolTickInfo / getTickLiquidity).
//! Кто вызывает: tests/depth_common/mod.rs.
//! От чего зависит: tests/native_common, arb::venues::bsc_dex::pools::{abi_v4, ticks, registry}.
//! Настройки: нет.

use std::collections::BTreeMap;
use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;

use arb::core::types::{PoolRef, PoolState};
use arb::venues::bsc_dex::pools::abi_v4::{IInfinityCl, IStateView};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

use super::super::native_common;
use super::{Rec, USDT, addr_word};

/// Пул CL (Infinity CL или Uniswap V4) из записи как запись реестра бота. None — встречный токен не USDT.
pub fn cl_entry(r: &Rec, p: &Value) -> Option<Entry> {
    let tag = p["tag"].as_str().unwrap();
    let infinity = p["kind"] == "infinity_cl";
    let key = if infinity {
        &r.got[&format!("{tag} CLPoolManager poolIdToPoolKey")]
    } else {
        &r.got[&format!("{tag} PositionManager poolKeys")]
    };
    let (c0, c1) = (addr_word(key, 0), addr_word(key, 1));
    let usdt = Address::from_str(USDT).unwrap();
    let coin = if c0 == usdt {
        c1
    } else if c1 == usdt {
        c0
    } else {
        return None;
    };
    let id = B256::from_str(p["id"].as_str().unwrap()).unwrap();
    let spacing = p["spacing"].as_i64().unwrap() as i32;
    let (sqrt, tick, protocol_fee, lp_fee, liquidity) = if infinity {
        let s = IInfinityCl::getSlot0Call::abi_decode_returns(
            &r.got[&format!("{tag} CLPoolManager getSlot0")],
        )
        .unwrap();
        let l = IInfinityCl::getLiquidityCall::abi_decode_returns(
            &r.got[&format!("{tag} CLPoolManager getLiquidity")],
        )
        .unwrap();
        (s.sqrtPriceX96, s.tick, s.protocolFee, s.lpFee, l)
    } else {
        let s = IStateView::getSlot0Call::abi_decode_returns(
            &r.got[&format!("{tag} StateView getSlot0")],
        )
        .unwrap();
        let l = IStateView::getLiquidityCall::abi_decode_returns(
            &r.got[&format!("{tag} StateView getLiquidity")],
        )
        .unwrap();
        (s.sqrtPriceX96, s.tick, s.protocolFee, s.lpFee, l)
    };
    let words = p["words"].as_array().unwrap();
    let words = (
        words[0].as_i64().unwrap() as i16,
        words[1].as_i64().unwrap() as i16,
    );
    let mut ticks = BTreeMap::new();
    let prefix = if infinity {
        format!("{tag} CLPoolManager getPoolTickInfo ")
    } else {
        format!("{tag} StateView getTickLiquidity ")
    };
    for (name, out) in r.got.range(prefix.clone()..) {
        let Some(t) = name.strip_prefix(&prefix) else {
            break;
        };
        let t: i32 = t.parse().unwrap();
        let (gross, net) = if infinity {
            let i = IInfinityCl::getPoolTickInfoCall::abi_decode_returns(out).unwrap();
            (i.liquidityGross, i.liquidityNet)
        } else {
            let i = IStateView::getTickLiquidityCall::abi_decode_returns(out).unwrap();
            (i.liquidityGross, i.liquidityNet)
        };
        ticks.insert(t, TickData { gross, net });
    }
    let kind = if infinity {
        "pancakeswap_infinity_cl"
    } else {
        "uniswap_v4"
    };
    let mut e = native_common::entry(kind, None);
    e.pool.reference = PoolRef::Singleton {
        manager: Address::ZERO,
        id,
    };
    e.pool.token0 = c0;
    e.pool.token1 = c1;
    e.pool.tick_spacing = Some(spacing);
    e.pool.state = PoolState::Cl {
        sqrt_price_x96: sqrt.to::<U256>(),
        tick: tick.as_i32(),
        liquidity,
        protocol_fee: protocol_fee.to(),
        lp_fee: lp_fee.to(),
    };
    e.ticker = p["coin"].as_str().unwrap().into();
    e.coin = coin;
    e.usdt = usdt;
    e.ticks = Some(TickMap {
        spacing,
        ticks,
        words,
    });
    Some(e)
}
