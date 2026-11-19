//! Файл: venues/bsc_dex/connection_gas.rs
//! Что делает: решение 433 — часть «Газ сети» в «Проверить подключение» площадки DEX: модель газа площадки
//! (dex.gas_mode, send/gas_mode.rs). legacy (BSC) — только название модели, узел не спрашивается. eip1559_l1
//! (Arbitrum) — NodeInterface своего узла (dex.node_interface, node/arb_gas.rs): оценка газа перевода 0 с кошелька
//! пробы на себя — весь газ, часть L2 и L1, baseFee L2. Ответ — описание или точная причина.
//! Кто вызывает: connection.rs (check).
//! От чего зависит: send/gas_mode.rs, node/arb_gas.rs, wallet/signer.rs (адрес кошелька пробы), ctx.rs.
//! Настройки: dex.gas_mode, dex.node_interface [площадка DEX]; узел — bsc.node_http_url.

use alloy_primitives::Bytes;

use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::node::arb_gas;
use super::send::gas_mode::{self, GasMode};
use super::wallet::signer;

/// Проверка модели газа площадки. Принимает: окружение. Возвращает: описание («legacy (gasPrice)» или части газа
/// Arbitrum) или текст причины (настройка не задана, кошелька пробы нет, узел не ответил, ответ не по ABI).
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
                Ok(format!(
                    "{}: перевод — газ {} (L2 {} + L1 {}), baseFee {} wei, оценка цены L1 {} wei",
                    GasMode::Eip1559L1.as_str(),
                    c.gas_estimate,
                    c.l2_gas(),
                    c.gas_for_l1,
                    c.base_fee,
                    c.l1_base_fee
                ))
            }
        }
    };
    run.await.map_err(|e| e.message)
}
