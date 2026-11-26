//! Файл: venues/bsc_dex/exec/tx_fee.rs
//! Что делает: решение 435 (задача 433-д) — цена газа транзакции кошелька по модели газа площадки dex.gas_mode
//! (send/gas_mode.rs): legacy — eth_gasPrice с надбавкой, не ниже минимума выбранных каналов (как было до решения
//! 435, exec/txs.rs); eip1559_l1 — цены типа 2 от базы головы (quote/gas.rs fees: потолок = база × запас + чаевые),
//! минимум каналов — по цене, которую транзакция заплатит (send/tx_price.rs floored).
//! Решение 437 (2): здесь только цены транзакции (потолок и чаевые за газ); затраты с разбиением газа L2/L1 —
//! exec/cycle_gas.rs.
//! Кто вызывает: exec/txs.rs (send_capped), exec/bundle_send.rs (только legacy — бандлы у сети типа 2 не отправляются).
//! От чего зависит: exec/txs.rs (Env), quote/gas.rs, send/gas_mode.rs, send/tx_price.rs, send/mod.rs (gas_floor).
//! Настройки: dex.gas_mode, send.gas_price_markup_pct, send.max_fee_margin_pct, send.priority_fee_gwei [площадка DEX].

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::quote::gas_base;
use super::super::send::gas_mode::{self, GasMode};
use super::super::send::ready::Ready;
use super::super::send::tx_price::{self, TxPrice};
use super::super::send::{self};
use super::txs::Env;

/// Цена газа транзакции. Принимает: окружение, выбранные каналы, чаевые сделки (решение 437 (3): выбранные долей
/// прибыли, exec/tip_cycle.rs; None — по режиму send.tip_mode; у legacy не читаются). Возвращает: цену legacy или типа 2.
/// Ошибки: NotConfigured (dex.gas_mode, надбавка, запас, чаевые), StaleData, BadResponse (число), узел.
pub async fn price(env: &Env, ready: &[Ready], tip: Option<u128>) -> GatewayResult<TxPrice> {
    let ctx = &env.ctx;
    let floor = send::gas_floor(ready);
    match gas_mode::read(ctx)? {
        GasMode::Legacy => Ok(TxPrice::Legacy(
            env.gas.wei(ctx, &env.heads).await?.max(floor),
        )),
        GasMode::Eip1559L1 | GasMode::Ethereum | GasMode::OpStack => {
            let (f, base) = env.gas.fees(ctx, &env.heads).await?;
            let f = match tip {
                Some(t) => gas_base::fees_with(ctx, base, t)?,
                None => f,
            };
            let f = tx_price::floored(f, floor, base).ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("цены газа {} / {} не помещаются", f.max_fee, f.tip),
                )
            })?;
            Ok(TxPrice::Eip1559(f, base))
        }
    }
}
