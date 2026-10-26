//! Файл: venues/bsc_dex/wallet/events_native.rs
//! Что делает: поступление нативной BNB на кошелёк по блокам (решение 76). У нативного перевода нет события Transfer
//! (решение 62), поэтому, только пока ожидается поступление BNB (wallet/inflows.rs — адрес депозита выдан), на каждой
//! новой голове читается баланс и число транзакций кошелька на блоке «голова − send.deposit_confirmations» и сравнивается
//! с прошлым прочитанным блоком:
//! - своих транзакций в этих блоках нет (nonce тот же) — разница баланса целиком чужая: рост — поступление (Deposit),
//!   падение — постороннее изменение;
//! - своя транзакция есть — её газ и сумма смешаны с поступлением, разложить однозначно нельзя: по событию не
//!   закрываем (одна запись в журнал), решает опрос статуса.
//!
//! Кто вызывает: wallet/events.rs.
//! От чего зависит: node/rpc.rs, node/call.rs, exec/native.rs, wallet/inflows.rs, num.rs, ctx.rs.
//! Настройки: dex.native_ticker, dex.native_wrapped (через exec/native.rs).
//!
//! eth_getBalance(address, block), eth_getTransactionCount(address, block) — execution-apis 16caee27 src/eth/state.yaml.

use alloy_primitives::{Address, U64, U256};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::{Asset, BalanceChange, BalanceKind};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::exec::native;
use super::super::node::call::block_tag;
use super::super::node::rpc;
use super::super::num;
use super::inflows::Inflows;

/// Что произошло между двумя прочитанными блоками.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Баланс тот же.
    Same,
    /// Своих транзакций нет, баланс вырос на столько wei.
    In(U256),
    /// Своих транзакций нет, баланс упал на столько wei.
    Out(U256),
    /// Есть своя транзакция — разница не раскладывается однозначно.
    Own,
}

/// Сравнивает (баланс, nonce) двух блоков. Ошибок нет.
pub fn decide(prev: (U256, u64), cur: (U256, u64)) -> Step {
    if cur.1 != prev.1 {
        return Step::Own;
    }
    match cur.0.cmp(&prev.0) {
        std::cmp::Ordering::Equal => Step::Same,
        std::cmp::Ordering::Greater => Step::In(cur.0 - prev.0),
        std::cmp::Ordering::Less => Step::Out(prev.0 - cur.0),
    }
}

/// Наблюдение за BNB: последний прочитанный блок, баланс и nonce.
#[derive(Default)]
pub struct Native {
    last: Option<(u64, U256, u64)>,
}

impl Native {
    /// Шаг на новой голове. Принимает: окружение, ожидаемые поступления, кошелёк, подтверждённый блок (голова −
    /// подтверждения). Возвращает: изменение баланса для ядра (None — нечего сообщать). Ошибки: узел.
    pub async fn step(
        &mut self,
        ctx: &Ctx,
        inflows: &Inflows,
        wallet: Address,
        block: u64,
    ) -> GatewayResult<Option<BalanceChange>> {
        let Some((ticker, w)) = native::wrapped(ctx).filter(|(_, w)| inflows.waiting(*w)) else {
            self.last = None;
            return Ok(None);
        };
        if self.last.is_some_and(|l| l.0 >= block) {
            return Ok(None);
        }
        let tag = block_tag(block);
        let bal: U256 = rpc::call(ctx, "eth_getBalance", json!([wallet, tag])).await?;
        let nonce: U64 = rpc::call(ctx, "eth_getTransactionCount", json!([wallet, tag])).await?;
        let prev = self.last.replace((block, bal, nonce.to()));
        let Some((from, pb, pn)) = prev else {
            return Ok(None);
        };
        let (delta, kind) = match decide((pb, pn), (bal, nonce.to())) {
            Step::Same => return Ok(None),
            Step::Own => {
                let msg = format!(
                    "{ticker}: в блоках {}–{block} есть своя транзакция кошелька — поступление {ticker} по балансу не раскладывается, ждём опрос статуса",
                    from + 1
                );
                ctx.log(Level::Info, "dex.balance_event", msg, None).await;
                return Ok(None);
            }
            Step::In(v) => (amount(v)?, BalanceKind::Deposit),
            Step::Out(v) => (-amount(v)?, BalanceKind::Other),
        };
        Ok(Some(BalanceChange {
            venue: ctx.venue(),
            asset: Asset {
                ticker,
                bsc_contract: Some(w),
            },
            delta,
            kind,
            own: false,
            tx_hash: None,
            at: Utc::now(),
            source: format!("баланс BNB кошелька, блоки {}–{block}", from + 1),
        }))
    }
}

/// Wei → BNB. Ошибка: сумма не помещается в Decimal (текст в ошибке узла не нужен — BadResponse).
fn amount(v: U256) -> GatewayResult<Decimal> {
    num::to_decimal(v, num::BNB_DECIMALS).ok_or_else(|| {
        crate::gateway::errors::GatewayError::new(
            crate::gateway::errors::ErrorCode::BadResponse,
            None,
            format!("изменение баланса BNB {v} wei не помещается в Decimal"),
        )
    })
}
