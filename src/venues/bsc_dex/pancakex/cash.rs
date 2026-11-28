//! Фактические активы в исполненной заявке. Для нативной выплаты нужен денежный CALL к получателю.
use super::Terms;
use crate::venues::bsc_dex::{exec::fill::moved_logs, node::abi::IERC20};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, U256};

alloy::sol! {
    struct Delivery {address token; uint256 amount; address recipient;}
    event Fill(bytes32 indexed orderHash,address indexed filler,address indexed swapper,uint256 nonce);
    event FillData(bytes32 indexed orderHash,address indexed inputToken,uint256 inputAmount,Delivery[] outputs);
}
/// Сумма принятых нативных CALL от исходного реактора до конечного кошелька в подтверждённой транзакции.
pub struct NativePayment {
    pub from: Address,
    pub to: Address,
    pub value: U256,
}
#[derive(Debug, Eq, PartialEq)]
pub struct Cash {
    pub paid: U256,
    pub received: U256,
    pub filler: Address,
}
impl Terms {
    pub fn cash(
        &self,
        logs: &[Log],
        native: &[NativePayment],
        uses_fill_data: bool,
    ) -> Option<Cash> {
        self.cash_bound(logs, native, uses_fill_data, self.min_output, false)
    }
    pub(super) fn cash_net(
        &self,
        logs: &[Log],
        native: &[NativePayment],
        uses_fill_data: bool,
        output: &super::assets::AssetCode,
    ) -> Option<Cash> {
        if !matches!(output, super::assets::AssetCode::TetherNet(_)) {
            return self.cash(logs, native, uses_fill_data);
        }
        self.cash_bound(
            logs,
            native,
            uses_fill_data,
            output.guaranteed_net(self.min_output),
            true,
        )
    }
    fn cash_bound(
        &self,
        logs: &[Log],
        native: &[NativePayment],
        uses_fill_data: bool,
        net_floor: U256,
        tether: bool,
    ) -> Option<Cash> {
        let roles = self.roles;
        let r = self.request;
        let mut fills = logs.iter().filter(|l| {
            l.address() == roles.reactor && l.topics().first() == Some(&Fill::SIGNATURE_HASH)
        });
        let first = fills.find(|l| l.topics().get(1) == Some(&self.order_hash))?;
        let event = Fill::decode_log_data(&first.inner.data).ok()?;
        if fills.any(|l| l.topics().get(1) == Some(&self.order_hash))
            || event.swapper != r.owner
            || event.nonce != self.nonce
            || event.filler == r.owner
            || event.filler.is_zero()
        {
            return None;
        }
        let filler = event.filler;
        let paid = moved_logs(logs, r.input, Some(r.owner), Some(filler));
        if paid.is_zero()
            || paid > self.maximum_input
            || paid != moved_logs(logs, r.input, Some(r.owner), None)
            || !moved_logs(logs, r.input, None, Some(r.owner)).is_zero()
        {
            return None;
        }
        let received = if r.output.is_zero() {
            let [transfer] = native else { return None };
            if transfer.from != roles.reactor || transfer.to != r.owner {
                return None;
            }
            transfer.value
        } else {
            let mut moves = logs.iter().filter(|l| {
                l.address() == r.output
                    && l.topics().first() == Some(&IERC20::Transfer::SIGNATURE_HASH)
                    && l.topics().get(2) == Some(&r.owner.into_word())
            });
            let receipt = IERC20::Transfer::decode_log_data(&moves.next()?.inner.data).ok()?;
            if moves.next().is_some()
                || receipt.from != filler
                || !moved_logs(logs, r.output, Some(r.owner), None).is_zero()
            {
                return None;
            }
            receipt.value
        };
        if received < net_floor {
            return None;
        }
        let mut data = logs.iter().filter(|l| {
            l.address() == roles.reactor
                && l.topics().first() == Some(&FillData::SIGNATURE_HASH)
                && l.topics().get(1) == Some(&self.order_hash)
        });
        match data.next() {
            Some(l) => {
                let d = FillData::decode_log_data(&l.inner.data).ok()?;
                if data.next().is_some() || d.inputToken != r.input || d.inputAmount != paid {
                    return None;
                }
                let mut owner = d.outputs.iter().filter(|o| o.recipient == r.owner);
                let bound = owner.next()?;
                if owner.next().is_some()
                    || bound.token != r.output
                    || if tether {
                        bound.amount < self.min_output || bound.amount < received
                    } else {
                        bound.amount != received
                    }
                {
                    return None;
                }
            }
            None if uses_fill_data => return None,
            None => {}
        }
        Some(Cash {
            paid,
            received,
            filler,
        })
    }
}
