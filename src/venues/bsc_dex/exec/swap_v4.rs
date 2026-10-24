//! Файл: venues/bsc_dex/exec/swap_v4.rs
//! Что делает: вход команды V4_SWAP / INFI_SWAP (0x10) Universal Router для обмена в одном пуле V4:
//! abi.encode(bytes actions, bytes[] params), действия [обмен с точным входом, SETTLE_ALL(вход, максимум),
//! TAKE_ALL(выход, минимум)] — минимум проверяется и в обмене (amountOutMinimum), и при получении (TAKE_ALL).
//! Путь через промежуточную монету (решение 52, правила SDK — research/flow/bsc_smart_router.md, раздел 3.1):
//! первый шаг — выход роутеру TAKE(валюта, ADDRESS_THIS, OPEN_DELTA); второй — SETTLE(валюта, CONTRACT_BALANCE,
//! payerIsUser = false) из баланса роутера (сумма шага: CONTRACT_BALANCE у второго шага пути; точная сумма доли —
//! у нативной продажи после WRAP_ETH, решение 62), обмен с amountIn = OPEN_DELTA (весь кредит), TAKE_ALL на кошелёк
//! (SETTLE 0x0b, TAKE 0x0e — Actions.sol; DeltaResolver._mapSettleAmount/_mapTakeAmount, CLRouterBase.sol#L26-L31;
//! Uniswap — v4-periphery 0c4e5bd3 V4Router.sol#L67-L74, #L86).
//! Кто вызывает: exec/plan.rs, exec/universal.rs.
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
use super::ur::{ADDRESS_THIS, Format, V4_SWAP};

const SWAP_EXACT_IN_SINGLE: u8 = 0x06;
const BIN_SWAP_EXACT_IN_SINGLE: u8 = 0x1c;
const SETTLE: u8 = 0x0b;
const SETTLE_ALL: u8 = 0x0c;
const TAKE: u8 = 0x0e;
const TAKE_ALL: u8 = 0x0f;

alloy::sol! {
    struct InfKey { address currency0; address currency1; address hooks; address poolManager; uint24 fee; bytes32 parameters; }
    struct InfSingle { InfKey poolKey; bool zeroForOne; uint128 amountIn; uint128 amountOutMinimum; bytes hookData; }
    struct UniKey { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
    struct UniSingle20 { UniKey poolKey; bool zeroForOne; uint128 amountIn; uint128 amountOutMinimum; bytes hookData; }
    struct UniSingle211 { UniKey poolKey; bool zeroForOne; uint128 amountIn; uint128 amountOutMinimum; uint256 minHopPriceX36; bytes hookData; }
}

/// Команда V4_SWAP/INFI_SWAP и вход. Принимает: пул V4, формат, обмен (токены, вход, минимум выхода), платит ли кошелёк
/// (false — роутер всем своим балансом), получает ли кошелёк (false — роутер). Ошибки: Internal (нет ключа или оба
/// шага с роутером), NoRoute (суммы больше uint128).
pub fn command(
    ctx: &Ctx,
    e: &Entry,
    f: Format,
    o: &Order,
    (from_wallet, to_wallet): (bool, bool),
) -> GatewayResult<(u8, Bytes)> {
    let (tin, tout, amount_in, min_out) = (o.token_in, o.token_out, o.amount_in, o.min_out);
    let key = e
        .key
        .as_ref()
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "у пула V4 нет ключа"))?;
    let big = || ctx.err(ErrorCode::NoRoute, "сумма больше uint128");
    // Из баланса роутера — amountIn = OPEN_DELTA (0): весь кредит после SETTLE (ActionConstants.sol#L11).
    let a_in: u128 = if from_wallet {
        amount_in.try_into().map_err(|_| big())?
    } else {
        0
    };
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
    let take_all = Bytes::from((tout, min_out).abi_encode_params());
    let (actions, params) = match (from_wallet, to_wallet) {
        (true, true) => (
            vec![action, SETTLE_ALL, TAKE_ALL],
            vec![
                Bytes::from(swap),
                Bytes::from((tin, amount_in).abi_encode_params()),
                take_all,
            ],
        ),
        (true, false) => (
            vec![action, SETTLE_ALL, TAKE],
            vec![
                Bytes::from(swap),
                Bytes::from((tin, amount_in).abi_encode_params()),
                Bytes::from((tout, ADDRESS_THIS, U256::ZERO).abi_encode_params()),
            ],
        ),
        (false, true) => (
            vec![SETTLE, action, TAKE_ALL],
            vec![
                Bytes::from((tin, amount_in, false).abi_encode_params()),
                Bytes::from(swap),
                take_all,
            ],
        ),
        (false, false) => {
            return Err(ctx.err(
                ErrorCode::Internal,
                "шаг V4 и из роутера, и роутеру — путь длиннее двух шагов",
            ));
        }
    };
    let actions = Bytes::from(actions);
    Ok((V4_SWAP, Bytes::from((actions, params).abi_encode_params())))
}
