//! Выплата собственного поручения PMM. Две стороны 0x Fill, выпуск/погашение WETH и терминальный перевод.
//! Для native ETH дополнительно требуется каноническая собственная транзакция и EOA-получатель без кода.
mod inclusion;
mod relay;
mod stored;
#[cfg(test)]
#[path = "receipt_tests.rs"]
mod tests;
use super::super::{exec::fill::moved_logs, node::abi::IERC20};
use super::wire::{
    Terms,
    abi::{Deposit, Fill, FillOrder, Withdrawal},
    asset,
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(in crate::venues::bsc_dex) struct Proof {
    #[serde(deserialize_with = "stored::terms")]
    pub terms: Terms,
    pub maker: Address,
    pub order_hash: B256,
    pub tx_hash: B256,
    pub gross: U256,
    pub net: U256,
    pub expiry: u64,
    pub fee: u16,
}

fn unique<E: SolEvent>(logs: &[Log], emitter: Address) -> Option<E> {
    let mut found = logs
        .iter()
        .filter(|l| l.address() == emitter && l.topics().first() == Some(&E::SIGNATURE_HASH));
    let v = E::decode_log_data_validate(&found.next()?.inner.data).ok()?;
    found.next().is_none().then_some(v)
}

impl Proof {
    pub(in crate::venues::bsc_dex) fn value(&self) -> U256 {
        if self.terms.input == self.terms.roles.weth.address {
            self.terms.spent
        } else {
            U256::ZERO
        }
    }
    /// Во время предпроверки и после включения сверяются одни и те же полные денежные события.
    pub(in crate::venues::bsc_dex) fn amounts(&self, logs: &[Log]) -> Option<(U256, U256)> {
        let t = &self.terms;
        let pmm = t.roles.pmm.address;
        let weth = t.roles.weth.address;
        let p = unique::<FillOrder>(logs, pmm)?;
        let x = unique::<Fill>(logs, t.roles.zero_exchange.address)?;
        if p.source != "0x v2"
            || p.transactionHash != self.tx_hash
            || p.orderHash != self.order_hash
            || p.userAddr != t.payer
            || p.receiverAddr != t.recipient
            || p.makerAddr != self.maker
            || p.takerAssetAddr != t.input
            || p.makerAssetAddr != t.output
            || p.takerAssetAmount != t.spent
            || p.makerAssetAmount != self.gross
            || p.feeFactor != self.fee
            || p.settleAmount != self.net
            || x.makerAddress != self.maker
            || x.feeRecipientAddress != t.payer
            || x.orderHash != self.order_hash
            || x.takerAddress != pmm
            || x.senderAddress != pmm
            || x.makerAssetFilledAmount != self.gross
            || x.takerAssetFilledAmount != t.spent
            || !x.makerFeePaid.is_zero()
            || !x.takerFeePaid.is_zero()
            || x.makerAssetData != asset(t.output)
            || x.takerAssetData != asset(t.input)
        {
            return None;
        }
        if logs.iter().any(|l| {
            l.removed
                || ([t.input, t.output].contains(&l.address())
                    && l.topics().first() == Some(&IERC20::Transfer::SIGNATURE_HASH)
                    && IERC20::Transfer::decode_log_data_validate(&l.inner.data).is_err())
        }) {
            return None;
        }
        let moved = |token, from, to| moved_logs(logs, token, from, to);
        if moved(t.input, Some(pmm), Some(self.maker)) != t.spent
            || moved(t.output, Some(self.maker), Some(pmm)) != self.gross
        {
            return None;
        }
        if t.input == weth {
            let d = unique::<Deposit>(logs, weth)?;
            if d.dst != pmm || d.wad != t.spent {
                return None;
            }
        } else if moved(t.input, Some(t.payer), Some(pmm)) != t.spent
            || moved(t.input, Some(t.payer), None) != t.spent
            || !moved(t.input, None, Some(t.payer)).is_zero()
        {
            return None;
        }
        let received = if t.output == weth {
            let w = unique::<Withdrawal>(logs, weth)?;
            if w.src != pmm || w.wad != self.net {
                return None;
            }
            // В pinned _settle сразу после withdraw средства переводятся тому же receiver.
            w.wad
        } else {
            let credit = moved(t.output, Some(pmm), Some(t.recipient));
            if moved(t.output, None, Some(t.recipient)) != credit
                || !moved(t.output, Some(t.recipient), None).is_zero()
            {
                return None;
            }
            credit
        };
        (received >= t.minimum && received <= self.net && !received.is_zero())
            .then_some((t.spent, received))
    }
}
