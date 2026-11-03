//! Файл: venues/bsc_dex/pools/load_woo.rs
//! Что делает: чтение состояния пары WooFi WooPPV2 (решение 122 F, 3) на блоке: состояние оракула по базовой монете
//! (`state(base)` — цена, спред, коэффициент, «цена пригодна»; откат — оракул на паузе или монета не поддержана →
//! цена непригодна), пауза общего контракта (`paused()`), его оракул (`wooracle()` — должен совпасть с dex.oracle) —
//! одним try_many (Multicall3); адрес реализации прокси — eth_getStorageAt слота EIP-1967 на том же блоке. Своей
//! математики обмена нет (развёрнутая реализация и новый оракул не опубликованы) — состояние нужно для свежести,
//! паузы и закреплённой реализации, выход считает квотер контракта (quote/quoter_calls.rs, query).
//! Кто вызывает: pools/load.rs (ветка Proto::WooPP); перечитывается на каждом новом блоке (pools/session.rs) — у
//! оракула нет событий.
//! От чего зависит: ctx.rs, node/call.rs, node/rpc.rs, abi_woo.rs.
//! Настройки: dex.oracle [DEX].
//!
//! Источники: abi_woo.rs (IWooPPV2.sol, IWooracleV2_2.sol woonetwork/WooPoolV2 f73a0f64; EIP-1967);
//! eth_getStorageAt — execution-apis 16caee27 src/eth/state.yaml.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::json;

use crate::core::types::{OraclePmmData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::{call, rpc};
use super::super::pools::families::Family;
use super::abi_woo::{IWooPP, IWooracle, implementation_slot};

/// Состояние пары на блоке. Принимает: окружение, вид, общий контракт, базовую монету, пару (token0, token1), тег.
/// Ошибки: узел, ответ не по ABI, не задана dex.oracle, оракул пула не тот (InvalidAddress).
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    coin: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let oracle = ctx.dex_addr("dex.oracle", f.id)?;
    let calls: Vec<(Address, Bytes)> = vec![
        (
            oracle,
            IWooracle::stateCall { base: coin }.abi_encode().into(),
        ),
        (pool, IWooPP::pausedCall {}.abi_encode().into()),
        (pool, IWooPP::wooracleCall {}.abi_encode().into()),
    ];
    let got = call::try_many(ctx, &calls, tag).await?;
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("WooPPV2 {pool}: {what}"));
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let ok = |i: usize| {
        got[i]
            .as_ref()
            .map_err(|r| bad(format!("вызов {i} откатился: {r}")))
    };
    let have = call::decode::<IWooPP::wooracleCall>(ok(2)?)?;
    if have != oracle {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            format!(
                "WooPPV2 {pool}: оракул пула {have:#x} ≠ dex.oracle {oracle:#x} — сверить и вписать"
            ),
        ));
    }
    let paused = call::decode::<IWooPP::pausedCall>(ok(1)?)?;
    let (price, spread, coeff, feasible) = match &got[0] {
        Ok(b) => {
            let s = call::decode::<IWooracle::stateCall>(b)?;
            (
                U256::from(s.price),
                U256::from(s.spread),
                U256::from(s.coeff),
                s.woFeasible,
            )
        }
        // Откат state(): оракул на паузе или монета не поддержана — цена непригодна, сделки нет.
        Err(_) => (U256::ZERO, U256::ZERO, U256::ZERO, false),
    };
    let word: B256 = rpc::call(
        ctx,
        "eth_getStorageAt",
        json!([pool, implementation_slot(), tag]),
    )
    .await?;
    Ok(PoolState::OraclePmm(Box::new(OraclePmmData {
        price,
        spread,
        coeff,
        feasible,
        paused,
        implementation: Address::from_word(word),
        base_is_token0: pair.0 == coin,
    })))
}
