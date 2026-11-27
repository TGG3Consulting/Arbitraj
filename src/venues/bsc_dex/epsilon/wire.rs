//! ABI фактического EpsilonRouter V6. Каждая доля платит собственному V3 пулу и возвращает выход роутеру.
use super::super::{
    ctx::Ctx,
    exec::{erc20_measure, fill, signed_call::Erc20Call},
    quote::white_state::Head,
    send::receipt::Receipt,
    wallet,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent, SolValue},
};
use alloy_primitives::{
    Address, B256, Bytes, U256,
    aliases::{U24, U160},
    keccak256,
};
use serde::{Deserialize, Serialize};

#[allow(clippy::too_many_arguments)]
mod abi {
    alloy::sol! {
        struct Routes {address[] targets;bytes[] callDatas;uint256[] amounts;}
        struct TokenPermissions {address token;uint256 amount;}
        struct Permit {TokenPermissions permitted;uint256 nonce;uint256 deadline;}
        struct Single {address tokenIn;address tokenOut;uint24 fee;address recipient;uint256 amountIn;uint256 amountOutMinimum;uint160 sqrtPriceLimitX96;}
        struct Order {address input;address output;uint256 amount;uint256 minimum;uint256 deadline;uint16 feeBps;uint24 firstTier;uint24 secondTier;uint256 firstAmount;}
        function swap(address tokenIn,address tokenOut,uint256 amountIn,uint256 expectedAmountOut,uint256 minAmountOut,bool unwrapOut,Routes routes,Permit permit,bytes signature) returns(uint256);
        function exactInputSingle(Single params) returns(uint256);
        function multicall(uint256 deadline,bytes[] data) returns(bytes[]);
        function trade(Order o) returns(uint256);
        event Swapped(address indexed user,address indexed tokenIn,address indexed tokenOut,uint256 amountIn,uint256 amountOut);
        event Settled(address indexed payer,address indexed input,address indexed output,bytes32 orderHash,uint256 spent,uint256 received,uint256 protocolFee);
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Plan {
    pub router: Address,
    pub providers: [Address; 2],
    pub pools: [Address; 2],
    pub tiers: [U24; 2],
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub first: U256,
    pub fee_bps: u16,
    pub minimum: U256,
    pub deadline: u64,
    pub wallet: Option<Address>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocean: Option<super::ocean_wire::Ocean>,
}
impl Plan {
    pub(super) fn fee(&self) -> Option<U256> {
        self.amount
            .checked_mul(U256::from(self.fee_bps))?
            .checked_div(U256::from(10000))
    }
    fn order(&self) -> abi::Order {
        abi::Order {
            input: self.input,
            output: self.output,
            amount: self.amount,
            minimum: self.minimum,
            deadline: U256::from(self.deadline),
            feeBps: self.fee_bps,
            firstTier: self.tiers[0],
            secondTier: self.tiers[1],
            firstAmount: self.first,
        }
    }
    fn legs(&self) -> Option<[U256; 2]> {
        Some([
            self.first,
            self.amount
                .checked_sub(self.fee()?)?
                .checked_sub(self.first)?,
        ])
    }
    pub(super) fn data(&self) -> Option<Bytes> {
        if let Some(offer) = &self.ocean {
            return offer.data(self);
        }
        let amounts = self.legs()?;
        if self.minimum.is_zero() || self.input == self.output || self.amount.is_zero() {
            return None;
        }
        if self.wallet.is_some() {
            return Some(abi::tradeCall { o: self.order() }.abi_encode().into());
        }
        let mut r = abi::Routes {
            targets: vec![],
            callDatas: vec![],
            amounts: vec![],
        };
        for (i, n) in amounts
            .into_iter()
            .enumerate()
            .filter(|(_, n)| !n.is_zero())
        {
            let o = abi::Single {
                tokenIn: self.input,
                tokenOut: self.output,
                fee: self.tiers[i],
                recipient: self.router,
                amountIn: n,
                amountOutMinimum: U256::ZERO,
                sqrtPriceLimitX96: U160::ZERO,
            };
            r.targets.push(self.providers[i]);
            r.amounts.push(n);
            r.callDatas.push(
                abi::multicallCall {
                    deadline: U256::from(self.deadline),
                    data: vec![abi::exactInputSingleCall { params: o }.abi_encode().into()],
                }
                .abi_encode()
                .into(),
            );
        }
        let permit = abi::Permit {
            permitted: abi::TokenPermissions {
                token: Address::ZERO,
                amount: U256::ZERO,
            },
            nonce: U256::ZERO,
            deadline: U256::ZERO,
        };
        Some(
            abi::swapCall {
                tokenIn: self.input,
                tokenOut: self.output,
                amountIn: self.amount,
                expectedAmountOut: self.minimum,
                minAmountOut: self.minimum,
                unwrapOut: false,
                routes: r,
                permit,
                signature: Bytes::new(),
            }
            .abi_encode()
            .into(),
        )
    }
    pub(super) fn signed(&self, chain: u64, owner: Address) -> Option<(Saved, Bytes)> {
        let data = self.data()?;
        Some((
            Saved {
                call: Erc20Call {
                    chain,
                    payer: owner,
                    module: self.wallet.unwrap_or(self.router),
                    calldata: keccak256(&data),
                },
                plan: self.clone(),
            },
            data,
        ))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Saved {
    pub call: Erc20Call,
    pub plan: Plan,
}
pub(super) fn one(logs: &[Log], addr: Address, topic: B256) -> Option<&Log> {
    let mut m = logs
        .iter()
        .filter(|x| x.address() == addr && x.topics().first() == Some(&topic));
    let first = m.next()?;
    m.next().is_none().then_some(first)
}
pub(super) fn venue_event(plan: &Plan, payer: Address, logs: &[Log]) -> bool {
    one(logs, plan.router, abi::Swapped::SIGNATURE_HASH)
        .and_then(|v| abi::Swapped::decode_log_data(&v.inner.data).ok())
        .is_some_and(|s| {
            (s.user, s.tokenIn, s.tokenOut, s.amountIn)
                == (payer, plan.input, plan.output, plan.amount)
        })
}
impl Saved {
    pub(in crate::venues::bsc_dex) fn wallet(&self, ctx: &Ctx) -> bool {
        ctx.chain_id() == Some(self.call.chain)
            && wallet::signer::address(ctx).ok() == Some(self.call.payer)
    }
    pub(in crate::venues::bsc_dex) fn amounts(&self, logs: &[Log]) -> Option<(U256, U256)> {
        let p = &self.plan;
        let who = self.call.payer;
        if let Some(api) = &p.ocean {
            return api.amounts(p, who, logs);
        }
        let signer = p.wallet.unwrap_or(who);
        let s = abi::Swapped::decode_log_data(
            &one(logs, p.router, abi::Swapped::SIGNATURE_HASH)?
                .inner
                .data,
        )
        .ok()?;
        if (s.user, s.tokenIn, s.tokenOut, s.amountIn) != (signer, p.input, p.output, p.amount) {
            return None;
        }
        let movements = |asset| {
            (
                fill::moved_logs(logs, asset, Some(who), None),
                fill::moved_logs(logs, asset, None, Some(who)),
            )
        };
        let (outgoing, incoming) = movements(p.input);
        let spent = outgoing.checked_sub(incoming)?;
        let (outgoing, incoming) = movements(p.output);
        let received = incoming.checked_sub(outgoing)?;
        if spent != p.amount || received < p.minimum {
            return None;
        }
        if let Some(w) = p.wallet {
            let x = abi::Settled::decode_log_data(
                &one(logs, w, abi::Settled::SIGNATURE_HASH)?.inner.data,
            )
            .ok()?;
            if (
                x.payer,
                x.input,
                x.output,
                x.spent,
                x.received,
                x.protocolFee,
                x.orderHash,
            ) != (
                who,
                p.input,
                p.output,
                spent,
                received,
                p.fee()?,
                keccak256(p.order().abi_encode()),
            ) {
                return None;
            }
        } else if s.amountOut != received {
            return None;
        }
        // В исходном ABI V3 обе проверенные фабрики эмитируют стандартный Swap.
        let topic = keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)");
        for (i, n) in p
            .legs()?
            .into_iter()
            .enumerate()
            .filter(|(_, n)| !n.is_zero())
        {
            let row = one(logs, p.pools[i], topic)?;
            if row.topics() != [topic, p.providers[i].into_word(), p.router.into_word()]
                || row.inner.data.data.len() != 160
            {
                return None;
            }
            let offset = if p.input < p.output { 0 } else { 32 };
            if U256::from_be_slice(&row.inner.data.data[offset..offset + 32]) != n {
                return None;
            }
        }
        Some((spent, received))
    }
    pub(super) async fn measure(
        &self,
        ctx: &Ctx,
        h: Head,
        data: &Bytes,
    ) -> GatewayResult<(U256, U256, u64)> {
        let (x, y, g, logs) = erc20_measure::run(
            ctx,
            h,
            self.plan.deadline,
            &self.call,
            (self.plan.input, self.plan.output),
            data,
        )
        .await?;
        if self.amounts(&logs) != Some((x, y)) {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Epsilon: не подтверждены рынок, расход и чистая выдача кошельку",
            ));
        }
        Ok((x, y, g))
    }
    pub(in crate::venues::bsc_dex) async fn included(
        &self,
        ctx: &Ctx,
        hash: B256,
        nonce: u64,
        r: &Receipt,
    ) -> GatewayResult<()> {
        self.call.included(ctx, hash, nonce, r).await?;
        if r.status.to::<u64>() == 1
            && let Some(api) = &self.plan.ocean
        {
            api.implementation
                .at_transaction(ctx, api.proxy, hash, r, None)
                .await?;
            if api.prepaid(&self.plan) == Some(true) {
                let fail = || {
                    ctx.err(ErrorCode::OutcomeUnknown,"OpenOcean: не подтверждена предоплата провайдера в исходном каноническом блоке")
                };
                api.funding
                    .as_ref()
                    .ok_or_else(fail)?
                    .included(ctx, hash, r)
                    .await
                    .ok_or_else(fail)?;
            }
        }
        if r.status.to::<u64>() == 1 && self.amounts(&r.logs).is_none() {
            return Err(ctx.err(
                ErrorCode::OutcomeUnknown,
                "Epsilon: каноническая квитанция не подтверждает исходный платёж",
            ));
        }
        Ok(())
    }
}
