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
use alloy::sol_types::{SolCall, SolEvent};
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
    /// Получатель, сумма, путь и минимум должны совпасть с сохранённым поручением даже при восстановлении.
    pub(super) fn signed_data(&self, data: &Bytes) -> bool {
        if keccak256(data) != self.data_hash {
            return false;
        }
        let deadline = match self.call.form {
            Form::OfficialEthIn => {
                IVistaFlow::swapExactETHForTokensSupportingFeeOnTransferTokensCall::abi_decode(data)
                    .map(|p| p.deadline)
            }
            Form::OfficialEthOut => {
                IVistaFlow::swapExactTokensForETHSupportingFeeOnTransferTokensCall::abi_decode(data)
                    .map(|p| p.deadline)
            }
            Form::RabbitEthIn => {
                IVistaFlow::swapExactETHForTokensCall::abi_decode(data).map(|p| p.deadline)
            }
            Form::RabbitEthOut => {
                IVistaFlow::swapExactTokensForETHCall::abi_decode(data).map(|p| p.deadline)
            }
            Form::ModuleEthOut => IVistaFlow::swapNativeCall::abi_decode(data).map(|p| p.deadline),
            Form::ModuleToken => return false,
        };
        deadline.is_ok_and(|deadline| {
            self.call.data(
                self.input,
                self.output,
                self.amount,
                self.minimum,
                self.wallet,
                deadline,
            ) == *data
        })
    }
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
                && self.signed_data(&t.input)
        }) {
            return Err(fail());
        }
        Ok(())
    }
    /// Возвращает реальное списание и чистую выплату владельцу. Газ учитывается отдельно по квитанции.
    pub(super) fn amounts(
        &self,
        logs: &[alloy::rpc::types::Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if wallet != self.wallet || input != self.input || output != self.output {
            return None;
        }
        let net = |token, credit| {
            let paid = fill::moved_logs(logs, token, Some(wallet), None);
            let got = fill::moved_logs(logs, token, None, Some(wallet));
            if credit {
                got.checked_sub(paid)
            } else {
                paid.checked_sub(got)
            }
        };
        let (spent, received) = match self.call.form {
            Form::RabbitEthIn if self.call.wrapped == input => {
                let deposited = native::moved_logs(logs, input, self.target, Side::Sell);
                if deposited != self.amount {
                    return None;
                }
                (deposited, net(output, true)?)
            }
            Form::RabbitEthOut if self.call.wrapped == output => (
                net(input, false)?,
                native::moved_logs(logs, output, self.target, Side::Buy),
            ),
            Form::OfficialEthIn if self.call.wrapped == input => {
                let wrapped = native::moved_logs(logs, input, self.target, Side::Sell);
                if wrapped.is_zero() || wrapped > self.amount {
                    return None;
                }
                (self.amount, net(output, true)?)
            }
            Form::OfficialEthOut if self.call.wrapped == output => {
                let gross = native::moved_logs(logs, output, self.target, Side::Buy);
                let received = gross.checked_sub(self.call.fee)?;
                (net(input, false)?, received)
            }
            Form::ModuleEthOut if self.call.wrapped == output => {
                let mut events = logs
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
