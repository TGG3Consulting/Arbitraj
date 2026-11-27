//! Защищённое исполнение PropAMM в одном слоте. Сверяет локальную подпись и обе дельты кошелька.
use super::{Builder, Ctx, Frame, Head, canonical, slot};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    exec::prop_wallet,
    pools::{families::Family, prop_venue},
};
use alloy::primitives::{Address, B256, Bytes, I256, U64, U256};
use alloy::sol_types::SolCall;
use serde_json::json;
alloy::sol! {function balanceOf(address account) external view returns(uint256);}
pub(super) async fn terms(
    ctx: &Ctx,
    f: &Family,
    r: &prop_venue::Roles,
    module: Address,
    h: &Head,
) -> GatewayResult<prop_wallet::Terms> {
    let at = json!({"blockHash":h.hash,"requireCanonical":true});
    let (hash, version) = tokio::try_join!(
        prop_venue::module(ctx, f, r, module, &at),
        canonical(ctx, h.hash, module, slot::slotVersionCall {})
    )?;
    if version != U256::from(2) {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Модуль не поддерживает однократное поручение в слоте",
        ));
    }
    let mut t = r.terms(ctx, module, false)?;
    t.module_hash = Some(hash);
    Ok(t)
}
pub(super) struct Order {
    pub terms: prop_wallet::Terms,
    pub payer: Address,
    pub tokens: (Address, Address),
    pub specified: I256,
    pub paid: U256,
    pub minimum: U256,
    pub leg: B256,
}
impl Order {
    pub fn proof(self, h: &Head, f: &Frame, builder: Builder) -> slot::Proof {
        let (input, output) = self.tokens;
        slot::Proof {
            builder: builder.name().into(),
            parent: h.hash,
            target: f.target,
            timestamp: f.timestamp,
            executor: self.terms.target,
            router: Some(self.terms.router),
            payer: self.payer,
            input,
            output,
            amount: self.specified,
            limit: if self.specified.is_negative() {
                self.paid
            } else {
                self.minimum
            },
            leg: self.leg,
            prop: Some(Box::new(prop_wallet::Proof {
                terms: self.terms,
                payer: self.payer,
                input,
                output,
                amount: self.paid,
                minimum: self.minimum,
                deadline: U256::from(f.timestamp),
            })),
        }
    }
}
pub(super) async fn simulate(
    ctx: &Ctx,
    h: &Head,
    f: &Frame,
    order: &slot::Proof,
    fees: Option<(u128, u128)>,
) -> GatewayResult<(U256, U256, u64)> {
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "PropAMM: не подтверждены обе суммы кошелька в заданном слоте",
        )
    };
    let balance = Bytes::from(
        balanceOfCall {
            account: order.payer,
        }
        .abi_encode(),
    );
    let view = |token| json!({"from":order.payer,"to":token,"data":balance});
    let mut trade = json!({"from":order.payer,"to":order.executor,"data":order.calldata()});
    if let Some((cap, tip)) = fees {
        trade["maxFeePerGas"] = json!(U256::from(cap));
        trade["maxPriorityFeePerGas"] = json!(U256::from(tip));
    }
    let rows = slot::run(
        ctx,
        h,
        f,
        json!([
            view(order.input),
            view(order.output),
            trade,
            view(order.input),
            view(order.output)
        ]),
    )
    .await?;
    let [a, b, swap, c, d] = rows.as_slice() else {
        return Err(bad());
    };
    if rows.iter().any(|v| v.status != U64::from(1)) || swap.gas_used.is_zero() {
        return Err(bad());
    }
    let read =
        |x: &super::SimCall| balanceOfCall::abi_decode_returns(&x.return_data).map_err(|_| bad());
    let spent = read(a)?.checked_sub(read(c)?).ok_or_else(bad)?;
    let received = read(d)?.checked_sub(read(b)?).ok_or_else(bad)?;
    let v = slot::executeAtSlotCall::abi_decode_returns(&swap.return_data).map_err(|_| bad())?;
    if (v.spent, v.received) != (spent, received) || !order.crossed(spent, received) {
        return Err(bad());
    }
    Ok((spent, received, swap.gas_used.to()))
}
