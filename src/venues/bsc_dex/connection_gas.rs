//! Файл: venues/bsc_dex/connection_gas.rs
//! Что делает: решение 433 — часть «Газ сети» в «Проверить подключение» площадки DEX: модель газа площадки
//! (dex.gas_mode, send/gas_mode.rs). legacy (BSC) — только название модели, узел не спрашивается. eip1559_l1
//! (Arbitrum) — NodeInterface своего узла (dex.node_interface, node/arb_gas.rs): оценка газа перевода 0 с кошелька
//! пробы на себя — весь газ, часть L2 и L1, baseFee L2. Ответ — описание или точная причина.
//! Решение 435 (433-д): отправка, отмена, котировка и перевод читают dex.gas_mode — не задана или не из списка — ошибка
//! этой части (пометка решения «433 (ревью)» снята: без настройки площадка не котирует и не отправляет). Перед выкладкой
//! — dex.gas_mode = legacy у bsc_dex на живом боте (docs/TASKS.md, «Решение 435»). У eip1559_l1 — ещё цены
//! транзакции типа 2 от baseFee узла: запас send.max_fee_margin_pct и чаевые send.priority_fee_gwei (quote/gas_base.rs).
//! Кто вызывает: connection.rs (check).
//! От чего зависит: send/gas_mode.rs, node/arb_gas.rs, quote/gas_base.rs, wallet/signer.rs (адрес кошелька пробы), ctx.rs.
//! Настройки: dex.gas_mode, dex.node_interface, send.max_fee_margin_pct, send.priority_fee_gwei [площадка DEX];
//! узел — bsc.node_http_url.

use alloy_primitives::Bytes;

use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::node::arb_gas;
use super::quote::gas_base;
use super::send::gas_mode::{self, GasMode};
use super::wallet::signer;

/// Проверка модели газа площадки. Принимает: окружение. Возвращает: описание («legacy (gasPrice)», части газа и
/// цены типа 2 у Arbitrum) или текст причины (dex.gas_mode не задана или не из списка, нет запаса или чаевых,
/// кошелька пробы нет, узел не ответил, ответ не по ABI).
pub async fn check(ctx: &Ctx) -> Result<String, String> {
    let run = async {
        match gas_mode::read(ctx)? {
            GasMode::Legacy => GatewayResult::Ok(format!(
                "{}: одна цена gasPrice, legacy-транзакция",
                GasMode::Legacy.as_str()
            )),
            GasMode::Eip1559L1 => {
                // Перевод 0 с кошелька пробы на себя — обычная транзакция без данных.
                let me = signer::address(ctx)?;
                let c = arb_gas::components(ctx, me, me, false, Bytes::new()).await?;
                // Решение 435: цены транзакции от этой baseFee — запас и чаевые заданы (иначе причина).
                let f = gas_base::fees(ctx, c.base_fee)?;
                Ok(format!(
                    "{}: перевод — газ {} (L2 {} + L1 {}), baseFee {} wei, оценка цены L1 {} wei; \
                     maxFeePerGas {} wei, чаевые {} wei",
                    GasMode::Eip1559L1.as_str(),
                    c.gas_estimate,
                    c.l2_gas(),
                    c.gas_for_l1,
                    c.base_fee,
                    c.l1_base_fee,
                    f.max_fee,
                    f.tip
                ))
            }
        }
    };
    run.await.map_err(|e| e.message)
}
