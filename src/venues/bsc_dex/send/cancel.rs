//! Файл: venues/bsc_dex/send/cancel.rs
//! Что делает: отмена транзакции, не попавшей в блок за send.max_wait_blocks: перевод 0 BNB себе с тем же nonce
//! и ценой газа выше прошлой на send.cancel_gas_bump_pct (и не ниже минимума выбранных каналов) — в те же каналы,
//! что и исходная (там она и ждёт). Попадёт в блок отмена или исходная — nonce занят, очередь кошелька свободна.
//! Решение 398: отмена уходит после исходной транзакции — настройки (надбавка, предел блока, запас газа) берутся из
//! SendCfg и подписант — из Watch, всё прочитано до отправки исходной; здесь настройки и ключ не читаются. ChainId
//! (решение 416, dex.evm_chain_id) — тоже из Watch.
//! Решение 435 (433-д): вид отмены — как у исходной (send/tx_price.rs): legacy — gasPrice выше прошлой; тип 2 (сеть
//! eip1559_l1) — обе цены (maxFeePerGas и чаевые) выше прошлых на ту же надбавку; потолок отмены сверяется по потолку
//! цены (send/track.rs).
//! Кто вызывает: send/track.rs.
//! От чего зависит: send/mod.rs (dispatch, gas_floor), send/tx.rs, send/tx_price.rs, send/track_util.rs (Watch).
//! Настройки: send.cancel_gas_bump_pct, send.bundle_max_blocks, send.gas_limit_margin_pct (все — через SendCfg).
//!
//! Замена транзакции с тем же nonce: у legacy-транзакции gasPrice должна быть ≥ старая × (100 + PriceBump) / 100,
//! иначе «replacement transaction underpriced» — узел BSC core/txpool/legacypool/list.go#L329-L351, legacypool.go#L150
//! (коммит 947ba1cb; research/flow/bsc_send.md, раздел 1.3). PriceBump у каждого узла и канала свой, поэтому надбавка —
//! настройка, а при отказе следующая попытка (через send.max_wait_blocks) ещё дороже. В приватных каналах замена
//! не документирована (там же) — отмена уходит во все каналы исходной транзакции.

use alloy_primitives::{B256, Bytes, TxKind, U256};

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::ready::Ready;
use super::track_util::Watch;
use super::tx::{self, SendCfg};
use super::tx_price::TxPrice;

/// Цена газа отмены: прошлая × (1 + надбавка/100), вверх до wei, не ниже минимума каналов (тип 2 — обе цены,
/// send/tx_price.rs bumped). Принимает: прошлую цену, настройки, каналы. Ошибки: BadResponse — число не считается.
pub fn price(ctx: &Ctx, prev: TxPrice, cfg: &SendCfg, ready: &[Ready]) -> GatewayResult<TxPrice> {
    prev.bumped(ctx, cfg.bump_pct, super::gas_floor(ready))
}

/// Отправляет отмену. Принимает: что отслеживается (каналы, счётчики, кошелёк, nonce, настройки, подписант — всё из
/// Watch, прочитано до отправки исходной), цену газа, текущий блок.
/// Возвращает: хеш отмены (принята хотя бы одним каналом или ответ канала не дошёл — она может быть в пуле).
/// Ошибки: оценка газа, подпись, все каналы ответили отказом.
pub async fn send(w: &Watch, price: TxPrice, head: u64) -> GatewayResult<B256> {
    let ctx = &w.ctx;
    let margin = w.cfg.gas_margin_pct;
    let gas = tx::limit_by(ctx, margin, w.from, w.from, &Bytes::new(), U256::ZERO).await?;
    let call = (TxKind::Call(w.from), U256::ZERO, Bytes::new());
    let signed = price.sign(
        ctx,
        &w.key,
        "Подпись отмены",
        w.chain_id,
        w.nonce,
        gas,
        call,
    )?;
    let max_block = head + w.cfg.bundle_max;
    super::dispatch(ctx, &w.stats, &w.ready, &signed.raw, None, max_block).await?;
    Ok(signed.hash)
}
