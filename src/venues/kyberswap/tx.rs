//! Файл: venues/kyberswap/tx.rs
//! Что делает: одна транзакция кошелька от подписи до итога через узел BSC (своя тонкая версия для kyberswap):
//! цена газа (eth_gasPrice + надбавка), лимит газа (eth_estimateGas + запас, заодно проверка, что вызов не откатится),
//! подпись legacy EIP-155, отправка (eth_sendRawTransaction), ожидание квитанции своим узлом до срока. Повторов и
//! отмен нет (стартовое правило Тиграна: отправлено и не исполнилось — больше не пробуем).
//! Nonce (решение 112): большее из ответа узла (eth_getTransactionCount «latest») и запомненного «следующего». Узел за
//! балансировщиком (публичный) отвечает с разных серверов, отстающий ещё не видит прошлую транзакцию в блоке и даёт
//! уже занятый номер — «nonce too low» (сделка FLOKI 02.11). Квитанция есть (в блоке или откат — номер занят) —
//! запоминаем nonce + 1. Квитанции нет — не запоминаем: следующий номер снова решает узел (транзакция могла выпасть).
//! Кто вызывает: trade.rs, transfer.rs.
//! От чего зависит: ctx.rs, node.rs (вызовы узла, Receipt), signer.rs.
//! Настройки: kyber.gas_*_pct, kyber.receipt_poll_ms, kyber.receipt_timeout_ms; узел — kyber.node_http_url.

use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use alloy_primitives::{Address, B256, Bytes, U256};

use crate::core::types::{ChainTx, SendMethod, TxOutcome};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::node::{self, Receipt};
use super::signer;

/// Итог транзакции: факт для ядра, квитанция (если попала в блок) и исход.
pub struct Done {
    pub chain: ChainTx,
    pub receipt: Option<Receipt>,
}

/// Подписывает и отправляет вызов, ждёт квитанции до kyber.receipt_timeout_ms. Принимает: окружение, отправителя,
/// адрес, данные, сумму BNB (wei), запомненный «следующий» nonce (state::Parts::nonce_next).
/// Возвращает: Done (исход — в блоке / откат / не попала).
/// Ошибки (транзакция не ушла): узел (nonce, газ, откат estimate), NotConfigured.
pub async fn send(
    ctx: &Ctx,
    next: &StdMutex<Option<u64>>,
    from: Address,
    to: Address,
    data: Bytes,
    value: U256,
) -> GatewayResult<Done> {
    let price = node::gas_price(ctx).await?;
    let gas = node::estimate_gas(ctx, from, to, &data, value).await?;
    let nonce = pick_nonce(ctx, next, node::tx_count(ctx, from).await?).await;
    let signed = signer::sign(
        ctx,
        signer::legacy(ctx, nonce, price, gas, to, value, data)?,
    )?;
    let hash = node::send_raw(ctx, &signed.raw).await?;
    let d = track(ctx, hash, nonce).await?;
    if d.receipt.is_some() {
        let mut n = next.lock().unwrap_or_else(|p| p.into_inner());
        *n = Some(n.map_or(nonce + 1, |k| k.max(nonce + 1)));
    }
    Ok(d)
}

/// Nonce транзакции: большее из ответа узла и запомненного. Узел отстал (ответ меньше) — запись в журнал.
/// Принимает: окружение, запомненный, ответ узла. Возвращает: nonce. Ошибок нет.
async fn pick_nonce(ctx: &Ctx, next: &StdMutex<Option<u64>>, node_says: u64) -> u64 {
    let known = *next.lock().unwrap_or_else(|p| p.into_inner());
    match known {
        Some(k) if k > node_says => {
            ctx.log(
                crate::storage::journal::Level::Info,
                "kyber.nonce",
                format!(
                    "Узел BSC отстаёт: ответил nonce {node_says}, свой следующий — {k}; берём {k}"
                ),
            )
            .await;
            k
        }
        _ => node_says,
    }
}

/// Ждёт квитанцию по хешу до срока. Принимает: окружение, хеш, nonce. Возвращает: Done.
/// Ошибки: только настройки (сроки); сбой узла при опросе квитанции повторяется в пределах срока.
async fn track(ctx: &Ctx, hash: B256, nonce: u64) -> GatewayResult<Done> {
    let poll = Duration::from_millis(ctx.int("kyber.receipt_poll_ms")?);
    let deadline = Duration::from_millis(ctx.int("kyber.receipt_timeout_ms")?);
    let started = Instant::now();
    loop {
        match node::receipt(ctx, hash).await {
            Ok(Some(r)) => return Ok(done(hash, nonce, Some(r))),
            Ok(None) => {}
            // Сбой узла при опросе — не итог: повторяем в пределах срока.
            Err(e) if started.elapsed() >= deadline => {
                ctx.log(
                    crate::storage::journal::Level::Warn,
                    "kyber.receipt",
                    format!("Квитанция {hash} не прочитана к сроку: {}", e.message),
                )
                .await;
                return Ok(done(hash, nonce, None));
            }
            Err(_) => {}
        }
        if started.elapsed() >= deadline {
            return Ok(done(hash, nonce, None));
        }
        tokio::time::sleep(poll).await;
    }
}

/// Факт транзакции из квитанции. Исход: статус 1 — в блоке, 0 — откат, нет квитанции — не попала. Ошибок нет.
fn done(hash: B256, nonce: u64, receipt: Option<Receipt>) -> Done {
    let outcome = match &receipt {
        Some(r) if r.status.to::<u64>() == 1 => TxOutcome::Included,
        Some(_) => TxOutcome::Reverted,
        None => TxOutcome::NotIncluded,
    };
    let chain = ChainTx {
        hash,
        nonce,
        channels: vec!["kyber.node".to_string()],
        method: SendMethod::Single,
        outcome,
        block: receipt.as_ref().map(|r| r.block_number.to()),
        gas_used: receipt.as_ref().map(|r| r.gas_used.to()),
        gas_bnb: receipt.as_ref().and_then(|r| r.gas_bnb()),
        tip_bnb: None,
        cancel_hash: None,
        cancel_gas_bnb: None,
    };
    Done { chain, receipt }
}
