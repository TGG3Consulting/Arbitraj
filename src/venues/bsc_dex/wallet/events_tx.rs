//! Файл: venues/bsc_dex/wallet/events_tx.rs
//! Что делает: для событий кошелька (решение 76) — чья транзакция (отправитель = адрес кошелька — своя: обмен, свой
//! вывод) и перевод токена в формате ядра (BalanceChange): тикер монеты подписки по адресу, сумма по decimals токена.
//! Обрабатываются только токены справочника монет бота (known): спам-токены и токен обёртки нативной монеты (WBNB — это
//! не BNB ядра; у нативной BNB событий Transfer нет, её считает events_native.rs) отсеиваются до запросов к узлу.
//! Кто вызывает: wallet/events.rs.
//! От чего зависит: node/rpc.rs, wallet/native_status.rs (Tx), exec/native.rs, tokens.rs, num.rs, ctx.rs.
//! Настройки: dex.native_ticker, dex.native_wrapped (через exec/native.rs).
//!
//! eth_getTransactionReceipt → from — execution-apis 16caee27 src/eth/transaction.yaml, src/schemas/transaction.yaml.

use std::collections::HashMap;

use alloy_primitives::{Address, B256};
use chrono::Utc;
use serde_json::json;

use crate::core::types::{Asset, BalanceChange, BalanceKind};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::native;
use super::super::node::rpc;
use super::super::num;
use super::super::send::track::Receipt;
use super::super::tokens::Tokens;
use super::events_log::Move;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WalletReceipt {
    from: Address,
    block_hash: B256,
    transaction_hash: B256,
    #[serde(flatten)]
    receipt: Receipt,
}

/// Отправители транзакций сессии: хеш → своя ли. В одной транзакции бывает несколько переводов кошелька (обмен) —
/// узел спрашивается один раз.
#[derive(Default)]
pub struct Senders {
    known: HashMap<B256, (Address, B256, Arc<Receipt>)>,
}

impl Senders {
    /// Своя ли транзакция. Принимает: окружение, хеш, кошелёк. Ошибки: узел; BadResponse — транзакции нет у узла.
    pub async fn own(&mut self, ctx: &Ctx, mv: &Move, wallet: Address) -> GatewayResult<bool> {
        let h = mv.tx;
        if self
            .known
            .get(&h)
            .is_none_or(|(_, block, _)| Some(*block) != mv.block_hash)
        {
            let tx: Option<WalletReceipt> =
                rpc::call(ctx, "eth_getTransactionReceipt", json!([h])).await?;
            let tx = tx
                .filter(|tx| {
                    tx.transaction_hash == h
                        && Some(tx.block_hash) == mv.block_hash
                        && tx.receipt.block_number.to::<u64>() == mv.block
                        && tx.receipt.status.to::<u64>() == 1
                })
                .ok_or_else(|| {
                    ctx.err(
                        ErrorCode::BadResponse,
                        format!("Квитанция не подтверждает блок события {h}"),
                    )
                })?;
            self.known
                .insert(h, (tx.from, tx.block_hash, Arc::new(tx.receipt)));
        }
        Ok(self.known[&h].0 == wallet)
    }

    pub fn receipt(&self, mv: &Move, wallet: Address) -> Option<Arc<Receipt>> {
        self.known
            .get(&mv.tx)
            .filter(|(_, block, r)| {
                Some(*block) == mv.block_hash
                    && r.logs.iter().any(|l| {
                        l.log_index == Some(mv.key.1)
                            && super::events_log::parse(l, wallet).ok().flatten().as_ref()
                                == Some(mv)
                    })
            })
            .map(|(_, _, r)| r.clone())
    }

    /// Забывает транзакции, чьих переводов больше не ждём. Принимает: хеши ещё ждущих. Ошибок нет.
    pub fn keep(&mut self, waiting: &[B256]) {
        self.known.retain(|h, _| waiting.contains(h));
    }
}

/// Токен из справочника монет бота и не обёртка нативной монеты. Ошибок нет.
pub fn known(ctx: &Ctx, tokens: &Tokens, token: Address) -> bool {
    let wrapped = native::wrapped(ctx).is_some_and(|(_, w)| w == token);
    !wrapped && tokens.ticker_of(token).is_some()
}

/// Вид изменения: поступление не своей транзакцией — Deposit, остальное — Other. Ошибок нет.
pub fn kind(inbound: bool, own: bool) -> BalanceKind {
    if inbound && !own {
        BalanceKind::Deposit
    } else {
        BalanceKind::Other
    }
}

/// Перевод токена → изменение баланса для ядра. Принимает: окружение, токены подписки, перевод, своя ли транзакция.
/// Ошибки: decimals токена не прочитаны (узел); BadResponse — токен не из справочника, сумма не помещается в Decimal.
pub async fn change(
    ctx: &Ctx,
    tokens: &Tokens,
    mv: &Move,
    own: bool,
) -> GatewayResult<BalanceChange> {
    let ticker = tokens.ticker_of(mv.token).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("токен {} не из справочника монет", mv.token),
        )
    })?;
    let dec = tokens.decimals(ctx, mv.token).await?;
    let amount = num::to_decimal(mv.units, dec).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!(
                "Перевод {} {ticker} (decimals {dec}) в {} не помещается в Decimal",
                mv.units, mv.tx
            ),
        )
    })?;
    Ok(BalanceChange {
        venue: ctx.venue(),
        asset: Asset {
            chain: None,
            ticker,
            contract: Some(mv.token),
        },
        delta: if mv.inbound { amount } else { -amount },
        kind: kind(mv.inbound, own),
        own,
        tx_hash: Some(mv.tx.to_string()),
        at: Utc::now(),
        source: format!("Transfer в блоке {}", mv.block),
        account: ctx.account.clone().map(crate::core::types::DexAccountId),
    })
}
