//! Публичные реквизиты подписанного EtherVista-вызова и денежный итог по квитанции.
//! Один и тот же разбор обслуживает штатное подтверждение и восстановление по исходному хешу.
use super::super::{ctx::Ctx, node::rpc, send::track::Receipt};
use super::{
    fill, native,
    vista_wire::{Call, Form, IVistaFlow},
};
use crate::{
    core::types::Side,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    pub call: Call,
    pub target: Address,
    pub wallet: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub data_hash: B256,
}

impl Proof {
    /// Сверяет адрес отправителя/цели, calldata и реальное value именно включённой транзакции.
    pub(crate) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Tx {
            hash: B256,
            block_hash: Option<B256>,
            from: Address,
            to: Option<Address>,
            value: U256,
            input: Bytes,
        }
        let fail = || {
            ctx.err(ErrorCode::OutcomeUnknown,"EtherVista: не подтверждены hash, блок, плательщик или сумма ETH подписанной транзакции")
        };
        if r.transaction_hash != Some(hash) || r.block_hash.is_none() {
            return Err(fail());
        }
        let tx: Option<Tx> = rpc::call(ctx, "eth_getTransactionByHash", json!([hash]))
            .await
            .map_err(|_| fail())?;
        if !tx.is_some_and(|t| {
            t.hash == hash
                && t.block_hash == r.block_hash
                && t.from == self.wallet
                && t.to == Some(self.target)
                && t.value == self.call.value(self.amount)
                && keccak256(t.input) == self.data_hash
        }) {
            return Err(fail());
        }
        Ok(())
    }
    /// Возвращает чистый вход и выход без включения газа и ранее имевшегося ETH/WETH у роутера.
    pub(super) fn amounts(
        &self,
        r: &Receipt,
        wallet: Address,
        input: Address,
        output: Address,
        side: Side,
    ) -> Option<(U256, U256)> {
        if wallet != self.wallet || input != self.input || output != self.output {
            return None;
        }
        let net = |token, credit| {
            let paid = fill::moved(r, token, Some(wallet), None);
            let got = fill::moved(r, token, None, Some(wallet));
            if credit {
                got.checked_sub(paid)
            } else {
                paid.checked_sub(got)
            }
        };
        let (spent, received) = match self.call.form {
            Form::OfficialEthIn if side == Side::Sell && self.call.wrapped == input => {
                let wrapped = native::moved(r, input, self.target, Side::Sell);
                if wrapped.is_zero() || wrapped > self.amount {
                    return None;
                }
                (self.amount, net(output, true)?)
            }
            Form::ModuleEthOut if side == Side::Buy && self.call.wrapped == output => {
                let mut events = r
                    .logs
                    .iter()
                    .filter(|l| l.address() == self.target)
                    .filter_map(|l| IVistaFlow::NativeSettled::decode_log_data(&l.inner.data).ok());
                let event = events.next()?;
                if events.next().is_some()
                    || event.payer != wallet
                    || event.recipient != wallet
                    || event.tokenIn != input
                    || event.tokenOut != output
                    || event.spent != net(input, false)?
                    || event.fee != self.call.fee
                {
                    return None;
                }
                (event.spent, event.received)
            }
            _ => return None,
        };
        (spent == self.amount && received >= self.minimum && !received.is_zero())
            .then_some((spent, received))
    }
}
