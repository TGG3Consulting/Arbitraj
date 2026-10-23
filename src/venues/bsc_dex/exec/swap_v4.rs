//! Файл: venues/bsc_dex/exec/swap_v4.rs
//! Что делает: вход команды V4_SWAP / INFI_SWAP (0x10) Universal Router для обмена в одном пуле V4:
//! abi.encode(bytes actions, bytes[] params), действия [обмен с точным входом, SETTLE_ALL(вход, максимум),
//! TAKE_ALL(выход, минимум)] — минимум проверяется и в обмене (amountOutMinimum), и при получении (TAKE_ALL).
//! Кто вызывает: exec/build.rs.
//! От чего зависит: exec/ur.rs (Format), pools/key.rs, pools/registry.rs.
//! Настройки: нет.
//!
//! Infinity: действия CL_SWAP_EXACT_IN_SINGLE 0x06, BIN_SWAP_EXACT_IN_SINGLE 0x1c, SETTLE_ALL 0x0c, TAKE_ALL 0x0f —
//! https://github.com/pancakeswap/infinity-periphery/blob/56a7a4f5076d57d5ed90c38a00421ecbac0d649c/src/libraries/Actions.sol#L19-L56 ;
//! параметры — src/pool-cl/interfaces/ICLRouterBase.sol#L11-L17, src/pool-bin/interfaces/IBinRouterBase.sol#L10-L16;
//! SETTLE_ALL / TAKE_ALL (валюта, сумма) — src/InfinityRouter.sol#L79-L90; payload — src/libraries/Planner.sol#L79-L81.
//! Uniswap V4: SWAP_EXACT_IN_SINGLE 0x06, SETTLE_ALL 0x0c, TAKE_ALL 0x0f — v4-periphery src/libraries/Actions.sol;
//! ExactInputSingleParams по версии UR: 2.0 — без minHopPriceX36, 2.1.1+ — с ним (0 = без проверки по шагу) —
//! https://github.com/Uniswap/sdks/blob/ecffab39cb8eba0ceb1a8c0052fd287f7765fcc8/sdks/v4-sdk/src/utils/v4Planner.ts#L86-L121 .

use alloy::sol_types::SolValue;
use alloy_primitives::aliases::U24;
use alloy_primitives::{Bytes, U256};

use crate::core::types::PoolKind;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::key::PoolKey;
use super::super::pools::registry::Entry;
use super::plan::Order;
use super::ur::{Format, V4_SWAP};

const SWAP_EXACT_IN_SINGLE: u8 = 0x06;
const BIN_SWAP_EXACT_IN_SINGLE: u8 = 0x1c;
const SETTLE_ALL: u8 = 0x0c;
const TAKE_ALL: u8 = 0x0f;

alloy::sol! {
    struct InfKey { address currency0; address currency1; address hooks; address poolManager; uint24 fee; bytes32 parameters; }
    struct InfSingle { InfKey poolKey; bool zeroForOne; uint128 amountIn; uint128 amountOutMinimum; bytes hookData; }
    struct UniKey { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
    struct UniSingle20 { UniKey poolKey; bool zeroForOne; uint128 amountIn; uint128 amountOutMinimum; bytes hookData; }
    struct UniSingle211 { UniKey poolKey; bool zeroForOne; uint128 amountIn; uint128 amountOutMinimum; uint256 minHopPriceX36; bytes hookData; }
}

/// Команда V4_SWAP/INFI_SWAP и вход. Принимает: пул V4, формат, обмен (токены, вход, минимум выхода).
/// Ошибки: Internal (нет ключа), NoRoute (суммы больше uint128).
pub fn command(ctx: &Ctx, e: &Entry, f: Format, o: &Order) -> GatewayResult<(u8, Bytes)> {
    let (tin, tout, amount_in, min_out) = (o.token_in, o.token_out, o.amount_in, o.min_out);
    let key = e
        .key
        .as_ref()
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "у пула V4 нет ключа"))?;
    let big = || ctx.err(ErrorCode::NoRoute, "сумма больше uint128");
    let a_in: u128 = amount_in.try_into().map_err(|_| big())?;
    let a_min: u128 = min_out.try_into().map_err(|_| big())?;
    let zero_for_one = tin == e.pool.token0;
    let (action, swap) = match (key, f) {
        (PoolKey::Infinity(k), _) => {
            let pk = InfKey {
                currency0: k.currency0,
                currency1: k.currency1,
                hooks: k.hooks,
                poolManager: k.manager,
                fee: U24::from(k.fee),
                parameters: k.parameters,
            };
            let p = InfSingle {
                poolKey: pk,
                zeroForOne: zero_for_one,
                amountIn: a_in,
                amountOutMinimum: a_min,
                hookData: Bytes::new(),
            };
            let action = if e.pool.kind == PoolKind::Bin {
                BIN_SWAP_EXACT_IN_SINGLE
            } else {
                SWAP_EXACT_IN_SINGLE
            };
            (action, p.abi_encode())
        }
        (PoolKey::Uniswap(k), fmt) => {
            let spacing = k.tick_spacing;
            let pk = UniKey {
                currency0: k.currency0,
                currency1: k.currency1,
                fee: U24::from(k.fee),
                tickSpacing: spacing,
                hooks: k.hooks,
            };
            let enc = if fmt == Format::Uniswap211 {
                UniSingle211 {
                    poolKey: pk,
                    zeroForOne: zero_for_one,
                    amountIn: a_in,
                    amountOutMinimum: a_min,
                    minHopPriceX36: U256::ZERO,
                    hookData: Bytes::new(),
                }
                .abi_encode()
            } else {
                UniSingle20 {
                    poolKey: pk,
                    zeroForOne: zero_for_one,
                    amountIn: a_in,
                    amountOutMinimum: a_min,
                    hookData: Bytes::new(),
                }
                .abi_encode()
            };
            (SWAP_EXACT_IN_SINGLE, enc)
        }
    };
    let actions = Bytes::from(vec![action, SETTLE_ALL, TAKE_ALL]);
    let params = vec![
        Bytes::from(swap),
        Bytes::from((tin, amount_in).abi_encode_params()),
        Bytes::from((tout, min_out).abi_encode_params()),
    ];
    Ok((V4_SWAP, Bytes::from((actions, params).abi_encode_params())))
}
