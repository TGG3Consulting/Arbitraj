//! Файл: venues/bsc_dex/quote/gas.rs
//! Что делает: цена газа для расчёта и отправки — eth_gasPrice узла с надбавкой send.gas_price_markup_pct;
//! запрашивается не чаще одного раза на блок (значение до прихода нового блока — из памяти).
//! Минимум канала отправки (channel.min_gas_price_gwei) применяется при отправке (exec/txs.rs) и в котировке (quote/mod.rs).
//! Кто вызывает: quote/mod.rs, exec/txs.rs, wallet/transfer.rs.
//! От чего зависит: node/rpc.rs, node/heads.rs, num.rs.
//! Настройки: send.gas_price_markup_pct [bsc_dex]; узел — bsc.node_http_url.
//!
//! eth_gasPrice → wei — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/fee_market.yaml ;
//! «цену газа брать eth_gasPrice + запас из настроек, без зашитого минимума» — research/flow/bsc_dex.md, раздел 5.

use std::sync::{Arc, Mutex};

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::node::rpc;
use super::super::num;

/// Цена газа с кэшем на блок. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct GasPrice {
    cache: Arc<Mutex<Option<(u64, U256)>>>,
}

impl GasPrice {
    /// Цена газа в wei с надбавкой. Ошибки: StaleData (нет свежего блока), NotConfigured (надбавка), узел.
    pub async fn wei(&self, ctx: &Ctx, heads: &Heads) -> GatewayResult<U256> {
        let head = heads.fresh(ctx)?;
        let cached = *self.cache.lock().unwrap_or_else(|p| p.into_inner());
        let base = match cached {
            Some((b, p)) if b == head.number => p,
            _ => {
                let p: U256 = rpc::call(ctx, "eth_gasPrice", json!([])).await?;
                *self.cache.lock().unwrap_or_else(|x| x.into_inner()) = Some((head.number, p));
                p
            }
        };
        let markup = ctx.dec("send.gas_price_markup_pct")?;
        let bad = || {
            ctx.err(
                ErrorCode::BadResponse,
                format!("Цена газа {base} wei не считается с надбавкой"),
            )
        };
        let v = num::to_decimal(base, 0).ok_or_else(bad)?
            * (Decimal::ONE + markup / Decimal::ONE_HUNDRED);
        num::to_units(v, 0).ok_or_else(bad)
    }
}
