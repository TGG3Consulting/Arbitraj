//! Оригинальный encodedOrder и подпись хранятся в памяти до истечения срока.
//! Источник формата — Gladius SDK и https://docs.rubicon.finance/developers/gladius-api.
use super::{abi::*, profile::Profile, *};
use alloy::sol_types::{SolCall, SolValue};
use serde::Deserialize;
use serde_json::json;
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct ApiOrder {
    pub chain_id: u64,
    pub order_status: String,
    pub encoded_order: Bytes,
    pub signature: Bytes,
    pub order_hash: B256,
}
pub(super) struct Offer {
    pub api: ApiOrder,
    pub order: Order,
    pub quantity: U256,
    pub debit: U256,
    pub expiry: u64,
}
impl Offer {
    pub(super) fn parse(api: ApiOrder, profile: &Profile, stamp: u64) -> Option<Self> {
        if api.chain_id != profile.chain
            || api.order_status != "open"
            || api.encoded_order.len() > 8192
            || api.signature.is_empty()
            || api.signature.len() > 4096
            || api.order_hash.is_zero()
        {
            return None;
        }
        let order = Order::abi_decode_validate(&api.encoded_order).ok()?;
        let [out] = order.outputs.as_slice() else {
            return None;
        };
        let expiry = u64::try_from(order.info.deadline).ok()?;
        if super::witness::hash(&order) != api.order_hash
            || order.abi_encode() != api.encoded_order.as_ref()
            || expiry <= stamp
            || order.info.reactor != profile.reactor.address
            || order.info.swapper.is_zero()
            || out.token.is_zero()
            || out.recipient.is_zero()
            || order.input.token.is_zero()
            || order.input.token == out.token
            || order.input.startAmount > order.input.endAmount
            || order.start > order.end
        {
            return None;
        }
        let now = U256::from(stamp);
        let quantity = if now >= order.end {
            order.input.endAmount
        } else if now <= order.start {
            order.input.startAmount
        } else {
            order.input.startAmount.checked_add(
                (order.input.endAmount - order.input.startAmount)
                    .checked_mul(now - order.start)?
                    .checked_div(order.end - order.start)?,
            )?
        };
        if quantity.is_zero() || order.threshold > quantity {
            return None;
        }
        Some(Self {
            api,
            order,
            quantity,
            debit: U256::ZERO,
            expiry,
        })
    }
    pub(super) fn pair(&self) -> (Address, Address) {
        (self.order.outputs[0].token, self.order.input.token)
    }
    pub(super) fn id(&self) -> (Address, U256) {
        (self.order.info.swapper, self.order.info.nonce)
    }
    pub(super) async fn quote(
        &self,
        ctx: &Ctx,
        profile: &Profile,
        quantity: U256,
        h: white_state::Head,
    ) -> GatewayResult<U256> {
        let bad = || {
            no(
                ctx,
                "Gladius: подпись, получатель или условия ордера не подтверждены",
            )
        };
        let op = IQuote::quoteCall {
            order: self.api.encoded_order.clone(),
            sig: self.api.signature.clone(),
            quantity,
        };
        let response:Bytes=rpc::call(ctx,"eth_call",json!([{"to":profile.quoter.address,"input":Bytes::from(op.abi_encode())},h.selector()])).await?;
        let r = IQuote::quoteCall::abi_decode_returns_validate(&response).map_err(|_| bad())?;
        if r.info.abi_encode() != self.order.info.abi_encode()
            || r.orderHash != self.api.order_hash
            || r.signature != self.api.signature
            || r.input.token != self.pair().1
            || r.input.amount != quantity
            || r.outputs.is_empty()
            || r.outputs[0].recipient != self.order.outputs[0].recipient
            || r.outputs
                .iter()
                .any(|o| o.token != self.pair().0 || o.recipient.is_zero())
        {
            return Err(bad());
        }
        r.outputs
            .iter()
            .try_fold(U256::ZERO, |a, o| a.checked_add(o.amount))
            .filter(|v| !v.is_zero())
            .ok_or_else(bad)
    }
}
