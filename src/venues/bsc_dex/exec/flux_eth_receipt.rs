//! Восстанавливает обе физические суммы по подписанному поручению и событию единственного менеджера.
use super::Proof;
use crate::gateway::errors::GatewayResult;
use crate::venues::bsc_dex::{
    ctx::Ctx,
    exec::{fill, signed_call},
    pools::abi_v4::{IInfinityBin, IInfinityCl},
    send::track::Receipt,
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256, U256};
impl Proof {
    pub(in crate::venues::bsc_dex::exec) async fn included(
        &self,
        ctx: &Ctx,
        hash: B256,
        r: &Receipt,
    ) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.market.router,
            self.value(),
            self.data(),
        )
        .await
    }
    pub(in crate::venues::bsc_dex) fn measured(
        &self,
        l: &[crate::venues::bsc_dex::check::simulate::SimLog],
    ) -> Option<(U256, U256)> {
        self.amounts(&signed_call::logs(l)?, self.payer, self.input, self.output)
    }
    pub(in crate::venues::bsc_dex::exec) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        let m = self.market;
        let buy = input == m.wrapped;
        if wallet != self.payer
            || (input, output) != (self.input, self.output)
            || (input, output)
                != if buy {
                    (m.wrapped, m.token)
                } else {
                    (m.token, m.wrapped)
                }
            || self.minimum.is_zero()
        {
            return None;
        }
        let topic = if m.bin {
            IInfinityBin::Swap::SIGNATURE_HASH
        } else {
            IInfinityCl::Swap::SIGNATURE_HASH
        };
        let mut events = logs
            .iter()
            .filter(|l| l.address() == m.manager && l.topic0() == Some(&topic));
        let log = &events.next()?.inner.data;
        if events.next().is_some() {
            return None;
        }
        let (id, router, x, y) = if m.bin {
            let ev = IInfinityBin::Swap::decode_log_data(log).ok()?;
            (ev.id, ev.sender, ev.amount0, ev.amount1)
        } else {
            let ev = IInfinityCl::Swap::decode_log_data(log).ok()?;
            (ev.id, ev.sender, ev.amount0, ev.amount1)
        };
        let (i, o) = if buy { (x, y) } else { (y, x) };
        if id != m.id || router != m.router || i >= 0 || o <= 0 {
            return None;
        }
        let (paid, received) = (U256::from(i.unsigned_abs()), U256::from(o as u128));
        let charged = fill::moved_logs(logs, m.token, Some(wallet), None);
        let credited = fill::moved_logs(logs, m.token, None, Some(wallet));
        let exact = if buy {
            credited.checked_sub(charged)? == received
        } else {
            charged.checked_sub(credited)? == paid
        };
        (exact && self.amount == paid && self.minimum <= received).then_some((paid, received))
    }
}
