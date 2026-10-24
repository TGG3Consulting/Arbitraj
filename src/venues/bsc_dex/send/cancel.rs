//! Файл: venues/bsc_dex/send/cancel.rs
//! Что делает: отмена транзакции, не попавшей в блок за send.max_wait_blocks: перевод 0 BNB себе с тем же nonce
//! и ценой газа выше прошлой на send.cancel_gas_bump_pct (и не ниже минимума выбранных каналов) — в те же каналы,
//! что и исходная (там она и ждёт). Попадёт в блок отмена или исходная — nonce занят, очередь кошелька свободна.
//! Кто вызывает: send/track.rs.
//! От чего зависит: send/mod.rs (dispatch, gas_floor), send/tx.rs, wallet/signer.rs, num.rs.
//! Настройки: send.cancel_gas_bump_pct, send.bundle_max_blocks (через SendCfg), send.gas_limit_margin_pct.
//!
//! Замена транзакции с тем же nonce: у legacy-транзакции gasPrice должна быть ≥ старая × (100 + PriceBump) / 100,
//! иначе «replacement transaction underpriced» — узел BSC core/txpool/legacypool/list.go#L329-L351, legacypool.go#L150
//! (коммит 947ba1cb; research/flow/bsc_send.md, раздел 1.3). PriceBump у каждого узла и канала свой, поэтому надбавка —
//! настройка, а при отказе следующая попытка (через send.max_wait_blocks) ещё дороже. В приватных каналах замена
//! не документирована (там же) — отмена уходит во все каналы исходной транзакции.

use alloy_primitives::{Address, B256, Bytes, U256};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::wallet::signer;
use super::ready::Ready;
use super::stats::Stats;
use super::tx::{self, SendCfg};

/// Цена газа отмены: прошлая × (1 + надбавка/100), вверх до wei, не ниже минимума каналов.
/// Принимает: прошлую цену, настройки, каналы. Ошибки: BadResponse — число не считается.
pub fn price(ctx: &Ctx, prev: U256, cfg: &SendCfg, ready: &[Ready]) -> GatewayResult<U256> {
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Цена газа отмены от {prev} wei не считается"),
        )
    };
    let p = num::to_decimal(prev, 0).ok_or_else(bad)?
        * (Decimal::ONE + cfg.bump_pct / Decimal::ONE_HUNDRED);
    let p = num::to_units(p.ceil(), 0).ok_or_else(bad)?;
    Ok(p.max(super::gas_floor(ready)))
}

/// Отправляет отмену. Принимает: каналы, счётчики, кошелёк, nonce, цену газа, текущий блок, настройки.
/// Возвращает: хеш отмены (принята хотя бы одним каналом или ответ канала не дошёл — она может быть в пуле).
/// Ошибки: оценка газа, подпись (ключ), все каналы ответили отказом.
#[allow(clippy::too_many_arguments)]
pub async fn send(
    ctx: &Ctx,
    stats: &Stats,
    ready: &[Ready],
    from: Address,
    nonce: u64,
    price: U256,
    head: u64,
    cfg: &SendCfg,
) -> GatewayResult<B256> {
    let gas = tx::limit(ctx, from, from, &Bytes::new(), U256::ZERO).await?;
    let t = tx::legacy(ctx, nonce, price, gas, from, U256::ZERO, Bytes::new())?;
    let signed = signer::sign(ctx, t)?;
    super::dispatch(ctx, stats, ready, &signed.raw, None, head + cfg.bundle_max).await?;
    Ok(signed.hash)
}
