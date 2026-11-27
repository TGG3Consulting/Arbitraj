//! Измерение единственного вызова кошелька в каноническом дочернем блоке.
use super::{
    Ctx, ErrorCode, GatewayResult,
    wire::{self, Offer, Quote},
};
use crate::venues::bsc_dex::{
    check::simulate::{self, Call},
    node::abi::IERC20,
};
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256, keccak256};
pub(super) struct Physical {
    pub input: Address,
    pub output: Address,
}

impl Physical {
    pub async fn measure(
        &self,
        ctx: &Ctx,
        head: super::white_state::Head,
        o: &Offer,
        q: &Quote,
        minimum: U256,
    ) -> GatewayResult<(U256, u64)> {
        let bad = || {
            ctx.err(
                ErrorCode::NoRoute,
                "Clipper: собственный расход, зачисление или событие обмена не подтвердились",
            )
        };
        let multi = if self.input.is_zero() || self.output.is_zero() {
            Some(ctx.addr("dex.multicall3")?)
        } else {
            None
        };
        let tx = |to, input: Vec<u8>| Call {
            from: o.owner,
            to,
            input: Bytes::from(input),
        };
        let balance = |asset: Address| {
            if asset.is_zero() {
                tx(
                    multi.unwrap_or_default(),
                    wire::getEthBalanceCall { owner: o.owner }.abi_encode(),
                )
            } else {
                tx(asset, IERC20::balanceOfCall { owner: o.owner }.abi_encode())
            }
        };
        let calls = [
            balance(self.input),
            balance(self.output),
            Call {
                from: o.owner,
                to: o.pool,
                input: o.data(q, self.input.is_zero(), self.output.is_zero()),
            },
            balance(self.input),
            balance(self.output),
        ];
        let values = if self.input.is_zero() {
            vec![(2, o.amount)]
        } else {
            Vec::new()
        };
        let base = simulate::Base::for_chain(Some(o.chain), head.number.to(), head.timestamp.to());
        let rows = simulate::run_anchored(ctx, &calls, &values, &[], base, Some(head.hash)).await?;
        let [a, b, step, c, d] = rows.as_slice() else {
            return Err(bad());
        };
        if !rows.iter().all(|r| r.ok()) {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Clipper: откат при действительных остатках, разрешении и сроке котировки",
            ));
        }
        let word = |r: &simulate::CallResult| {
            (r.return_data.len() == 32).then(|| U256::from_be_slice(&r.return_data))
        };
        let credited = word(d)
            .and_then(|v| v.checked_sub(word(b)?))
            .ok_or_else(bad)?;
        if word(a).and_then(|v| v.checked_sub(word(c)?)) != Some(o.amount) || credited < minimum {
            return Err(bad());
        }
        let topic = keccak256("Swapped(address,address,address,uint256,uint256,bytes)");
        let mut events = step
            .logs
            .iter()
            .filter(|l| l.address == o.pool && l.topics.first() == Some(&topic));
        let evt = events.next().ok_or_else(bad)?;
        let [_, input, output, recipient] = evt.topics.as_slice() else {
            return Err(bad());
        };
        let terms =
            <(U256, U256, Bytes)>::abi_decode_params_validate(&evt.data).map_err(|_| bad())?;
        let aliases = |topic: &alloy_primitives::B256, physical: Address, token: Address| {
            *topic == token.into_word() || *topic == physical.into_word()
        };
        if events.next().is_some()
            || !aliases(input, self.input, o.input)
            || !aliases(output, self.output, o.output)
            || *recipient != o.owner.into_word()
            || terms.0 != o.amount
            || terms.1 != o.received
            || !terms.2.is_empty()
        {
            return Err(bad());
        }
        let transfer = keccak256("Transfer(address,address,uint256)");
        let paid = |asset: Address, from: Address, to: Address, amt: U256| {
            asset.is_zero()
                || step.logs.iter().any(|l| {
                    l.address == asset
                        && l.topics == [transfer, from.into_word(), to.into_word()]
                        && l.data.len() == 32
                        && U256::from_be_slice(&l.data) == amt
                })
        };
        if !paid(self.input, o.owner, o.pool, o.amount)
            || !paid(self.output, o.pool, o.owner, credited)
        {
            return Err(bad());
        }
        Ok((credited, step.gas_used.to()))
    }
}
