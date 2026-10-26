//! Файл: venues/bsc_dex/wallet/events_tx.rs
//! Что делает: для событий кошелька (решение 76) — чья транзакция (отправитель = адрес кошелька — своя: обмен, свой
//! вывод) и перевод токена в формате ядра (BalanceChange): тикер монеты подписки по адресу, сумма по decimals токена.
//! Токен обёртки нативной монеты (WBNB) — это не BNB ядра (у нативной BNB событий Transfer нет, её считает
//! events_native.rs), поэтому у него и у токенов вне подписки тикер — адрес токена: зачислением BNB они не станут.
//! Кто вызывает: wallet/events.rs.
//! От чего зависит: node/rpc.rs, wallet/native_status.rs (Tx), exec/native.rs, tokens.rs, num.rs, ctx.rs.
//! Настройки: dex.native_ticker, dex.native_wrapped (через exec/native.rs).
//!
//! eth_getTransactionByHash → from — execution-apis 16caee27 src/eth/transaction.yaml, src/schemas/transaction.yaml.

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
use super::super::tokens::Tokens;
use super::events_log::Move;
use super::native_status::Tx;

/// Отправители транзакций сессии: хеш → своя ли. В одной транзакции бывает несколько переводов кошелька (обмен) —
/// узел спрашивается один раз.
#[derive(Default)]
pub struct Senders {
    known: HashMap<B256, bool>,
}

impl Senders {
    /// Своя ли транзакция. Принимает: окружение, хеш, кошелёк. Ошибки: узел; BadResponse — транзакции нет у узла.
    pub async fn own(&mut self, ctx: &Ctx, h: B256, wallet: Address) -> GatewayResult<bool> {
        if let Some(own) = self.known.get(&h) {
            return Ok(*own);
        }
        let tx: Option<Tx> = rpc::call(ctx, "eth_getTransactionByHash", json!([h])).await?;
        let tx = tx.ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("лог Transfer есть, а транзакции {h} у узла нет"),
            )
        })?;
        let own = tx.from == wallet;
        self.known.insert(h, own);
        Ok(own)
    }

    /// Забывает транзакции, чьих переводов больше не ждём. Принимает: хеши ещё ждущих. Ошибок нет.
    pub fn keep(&mut self, waiting: &[B256]) {
        self.known.retain(|h, _| waiting.contains(h));
    }
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
/// Ошибки: decimals токена не прочитаны (узел); BadResponse — сумма не помещается в Decimal.
pub async fn change(
    ctx: &Ctx,
    tokens: &Tokens,
    mv: &Move,
    own: bool,
) -> GatewayResult<BalanceChange> {
    let wrapped = native::wrapped(ctx).is_some_and(|(_, w)| w == mv.token);
    let ticker = tokens
        .ticker_of(mv.token)
        .filter(|_| !wrapped)
        .unwrap_or_else(|| mv.token.to_string());
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
            ticker,
            bsc_contract: Some(mv.token),
        },
        delta: if mv.inbound { amount } else { -amount },
        kind: kind(mv.inbound, own),
        own,
        tx_hash: Some(mv.tx.to_string()),
        at: Utc::now(),
        source: format!("Transfer в блоке {}", mv.block),
    })
}
