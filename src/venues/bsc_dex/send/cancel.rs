//! Файл: venues/bsc_dex/send/cancel.rs
//! Что делает: отмена транзакции, не попавшей в блок за send.max_wait_blocks: перевод 0 BNB себе с тем же nonce
//! и ценой газа выше прошлой на send.cancel_gas_bump_pct (и не ниже минимума выбранных каналов) — в те же каналы,
//! что и исходная (там она и ждёт). Попадёт в блок отмена или исходная — nonce занят, очередь кошелька свободна.
//! Решение 398: отмена уходит после исходной транзакции — настройки (надбавка, предел блока, запас газа) берутся из
//! SendCfg и подписант — из Watch, всё прочитано до отправки исходной; здесь настройки и ключ не читаются. ChainId
//! (решение 416, dex.evm_chain_id) — тоже из Watch.
//! Кто вызывает: send/track.rs.
//! От чего зависит: send/mod.rs (dispatch, gas_floor), send/tx.rs, send/track_util.rs (Watch), wallet/signer.rs, num.rs.
//! Настройки: send.cancel_gas_bump_pct, send.bundle_max_blocks, send.gas_limit_margin_pct (все — через SendCfg).
//!
//! Замена транзакции с тем же nonce: у legacy-транзакции gasPrice должна быть ≥ старая × (100 + PriceBump) / 100,
//! иначе «replacement transaction underpriced» — узел BSC core/txpool/legacypool/list.go#L329-L351, legacypool.go#L150
//! (коммит 947ba1cb; research/flow/bsc_send.md, раздел 1.3). PriceBump у каждого узла и канала свой, поэтому надбавка —
//! настройка, а при отказе следующая попытка (через send.max_wait_blocks) ещё дороже. В приватных каналах замена
//! не документирована (там же) — отмена уходит во все каналы исходной транзакции.

use alloy_primitives::{B256, Bytes, U256};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::wallet::signer;
use super::ready::Ready;
use super::track_util::Watch;
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

/// Отправляет отмену. Принимает: что отслеживается (каналы, счётчики, кошелёк, nonce, настройки, подписант — всё из
/// Watch, прочитано до отправки исходной), цену газа, текущий блок.
/// Возвращает: хеш отмены (принята хотя бы одним каналом или ответ канала не дошёл — она может быть в пуле).
/// Ошибки: оценка газа, подпись, все каналы ответили отказом.
pub async fn send(w: &Watch, price: U256, head: u64) -> GatewayResult<B256> {
    let ctx = &w.ctx;
    let margin = w.cfg.gas_margin_pct;
    let gas = tx::limit_by(ctx, margin, w.from, w.from, &Bytes::new(), U256::ZERO).await?;
    let t = tx::legacy(
        ctx,
        w.chain_id,
        w.nonce,
        price,
        gas,
        w.from,
        U256::ZERO,
        Bytes::new(),
    )?;
    let signed = signer::sign_with(&w.key, t)
        .map_err(|e| ctx.err(ErrorCode::Internal, format!("Подпись отмены: {e}")))?;
    let max_block = head + w.cfg.bundle_max;
    super::dispatch(ctx, &w.stats, &w.ready, &signed.raw, None, max_block).await?;
    Ok(signed.hash)
}
