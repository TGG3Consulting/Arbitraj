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
//! Решение 436: отмена уходит без условия eth_sendRawTransactionConditional (send/sequencer_cond.rs) — обычной отправкой.
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

/// Для geth и OP-geth оба поля замены строго больше исходных, даже при нулевых чаевых.
/// Свежая baseFee проверяется до установленного лимита send.cancel_max_gas_gwei.
/// https://github.com/ethereum/go-ethereum/blob/master/core/txpool/legacypool/list.go
pub async fn at(w: &Watch, previous: TxPrice, block: u64) -> GatewayResult<TxPrice> {
    let candidate = price(&w.ctx, previous, &w.cfg, &w.ready)?;
    if !matches!(w.ctx.place.id, "eth_dex" | "base_dex") {
        return Ok(candidate);
    }
    let (TxPrice::Eip1559(old, _), TxPrice::Eip1559(bumped, _)) = (previous, candidate) else {
        return Err(w.ctx.err(
            crate::gateway::errors::ErrorCode::BadResponse,
            "для замены Ethereum/Base требуется type-2",
        ));
    };
    let base = super::super::quote::gas_base::fetch(&w.ctx, block).await?;
    let f = strict(old, bumped, base, super::gas_floor(&w.ready)).ok_or_else(|| {
        w.ctx.err(
            crate::gateway::errors::ErrorCode::BadResponse,
            "невозможно повысить оба поля комиссии type-2 для замены",
        )
    })?;
    Ok(TxPrice::Eip1559(f, base))
}

/// Строгое увеличение обоих полей, достаточная эффективная ставка и покрытие текущей базы.
fn strict(
    old: super::tx_1559::Fees,
    mut bumped: super::tx_1559::Fees,
    base: U256,
    floor: U256,
) -> Option<super::tx_1559::Fees> {
    bumped.tip = bumped.tip.max(old.tip.checked_add(1)?);
    bumped.max_fee = bumped.max_fee.max(old.max_fee.checked_add(1)?);
    super::tx_price::floored(bumped, floor, base)
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
    if let Some(oracle) = w.op_oracle {
        let unsigned = price.unsigned(ctx, w.chain_id, w.nonce, gas, call.clone())?;
        let extra = oracle.for_tx(ctx, head, gas, unsigned, false).await?;
        let need = price
            .cap()
            .saturating_mul(U256::from(gas))
            .saturating_add(extra);
        super::super::exec::gas_fund::ensure(ctx, w.from, need).await?;
    }
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
    // Решение 436: отмена — без условия секвенсора (её дело занять nonce; время головы не нужно).
    let terms = super::sequencer_cond::Terms::default();
    super::dispatch(ctx, &w.stats, &w.ready, &signed.raw, None, max_block, terms).await?;
    Ok(signed.hash)
}

#[cfg(test)]
mod tests {
    use super::super::tx_1559::Fees;
    use super::*;
    /// Первая записанная транзакция Ethereum имеет нулевые чаевые. Для замены их требуется поднять минимум на wei.
    #[test]
    fn recorded_zero_tip_can_be_replaced() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/data/base_ethereum_gas_2026-11-26/ethereum-type2.json"
        ))
        .unwrap();
        let row = &fixture["txs"][0];
        let n = |key: &str| row[key].as_u64().unwrap();
        let old = Fees {
            tip: n("max_priority_fee_per_gas_wei").into(),
            max_fee: n("max_fee_per_gas_wei").into(),
        };
        assert_eq!(old.tip, 0);
        let next = strict(old, old, U256::from(n("base_fee_wei")), U256::ZERO).unwrap();
        assert_eq!(next.tip, 1);
        assert!(next.max_fee > old.max_fee);
    }
}
