//! Файл: venues/bsc_dex/pools/fee_fresh.rs
//! Что делает: решение 281 — комиссия пула V3-форка, которую меняют без пересоздания пула (SquadSwap V3: `fee()` пула =
//! `poolManager.getFee(pool, defaultFee)`, SquadV3Pool.sol L136-L137; меняет оператор менеджера setFee / setFeeBatch /
//! toggleCustomFee, SquadV3PoolManager.sol L43-L66; пул берёт `fee()` в каждом шаге swap — SquadV3Pool.sol L673-L680).
//! Без этой настройки бот читает fee() один раз при проверке пула и считал бы обмен по старой комиссии. У DEX с
//! dex.v3_fee_source = pool_fee_each_read к каждому чтению головы пула V3 (slot0 + liquidity — пачкой load_batch и по
//! одному load.rs) добавляется `fee()` того же пула на том же блоке; прочитанная комиссия идёт в запись (load.rs
//! renewed / built). Предторговая верификация (quote/verify.rs) перечитывает пулы маршрута тем же путём — сделка
//! считается по комиссии блока верификации, устаревшей комиссии в расчёте сделки быть не может. События менеджера
//! (fee_watch.rs) — ускорение для сигналов между чтениями, а не единственный источник.
//! Не задана (или pool / factory_swap_fee) — вызова нет, поведение и число обращений к узлу прежние.
//! Кто вызывает: pools/load_batch_v2v3.rs (head), pools/load_batch.rs (split), pools/load.rs (V3 по одному).
//! От чего зависит: ctx.rs, node/call.rs, abi_check.rs (fee()), families.rs, load_batch.rs (Miss).
//! Настройки: dex.v3_fee_source [DEX] (значение pool_fee_each_read; необязательная).
//!
//! Источник (verified, Sourcify partial_match, chain 56): SquadV3Pool.sol (пул 0x9fb9815ca1417b06466aaa86521a28ba83d0bd33,
//! sha256 88dd3da0…9283); SquadV3PoolManager.sol (0x391eaa90f931c6330132efe6c73ebdf77d782ef5, sha256 cbdbe8fd…cdc9).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_check::IPoolImmutables as P;
use super::families::{Family, Proto};
use super::load_batch::Miss;

/// Значение dex.v3_fee_source, при котором fee() пула читается при каждом чтении его состояния.
pub const EACH_READ: &str = "pool_fee_each_read";

/// Читать ли fee() с каждой головой: вид V3 (ABI Uniswap или PancakeSwap) и dex.v3_fee_source = pool_fee_each_read.
/// Не задана — нет (поведение прежнее). Ошибок нет.
pub fn on(ctx: &Ctx, f: &Family) -> bool {
    matches!(f.proto, Proto::UniswapV3 | Proto::PancakeV3)
        && ctx
            .dex_text("dex.v3_fee_source", f.id)
            .is_ok_and(|v| v == EACH_READ)
}

/// Вызов fee() пула для пачки (последним в голове пула). Ошибок нет.
pub fn call(pool: Address) -> (Address, Bytes) {
    (pool, Bytes::from(P::feeCall {}.abi_encode()))
}

/// Ответы головы пула пачкой → (ответы без fee(), комиссия). Без настройки — ответы как есть и None. Принимает: включено
/// ли (on), ответы. Ошибки: причина (нет ответа fee() или он не по ABI) — пул не прочитан.
pub fn split(on: bool, b: &[Bytes]) -> Result<(&[Bytes], Option<u32>), Miss> {
    if !on {
        return Ok((b, None));
    }
    let Some((last, rest)) = b.split_last() else {
        return Err(super::load_batch::miss(
            "нет ответа fee() пула (решение 281)",
        ));
    };
    let fee = call::decode::<P::feeCall>(last)?;
    Ok((rest, Some(fee.to())))
}

/// fee() пула на блоке по одному (load.rs). Ошибки: узел, ответ не по ABI.
pub async fn read(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<u32> {
    let fee = call::view(ctx, pool, &P::feeCall {}, tag).await?;
    Ok(fee.to())
}
