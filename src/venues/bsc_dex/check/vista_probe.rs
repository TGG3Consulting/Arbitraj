//! Налоговая проба EtherVista по официальному ETH ABI поверх одного канонического блока.
//! WETH из источника пробы разворачивается в этом же кадре. Выход ETH измеряется на кошельке с учётом аванса.
use super::super::{
    ctx::Ctx,
    exec::{
        plan::Order,
        vista_wire::{Call as VistaCall, Form},
    },
    pools::{ethervista::VistaMarket, registry::Entry},
    quote::{
        vista_pricing,
        white_state::{self, Head},
    },
    wallet::approve::{self, Approval},
};
use super::{probe_calls as pc, simulate::Call};
use crate::gateway::errors::GatewayResult;
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
sol! {interface IProbeAsset {
    function withdraw(uint256 amount) external;
    function deposit() external payable;
    function getEthBalance(address owner) external view returns(uint256);
}}
pub(super) struct Probe {
    market: Option<VistaMarket>,
    state: crate::core::types::PoolState,
    wrapped: Address,
    pub head: Head,
    multi: Address,
}
impl Probe {
    /// Только собственный официальный маршрут. При остатке на роутере остаётся существующая проба модуля.
    pub async fn of(
        ctx: &Ctx,
        e: &Entry,
        number: u64,
        wallet: Address,
    ) -> GatewayResult<Option<Self>> {
        let head = white_state::head(ctx, &format!("{number:#x}")).await?;
        if super::super::exec::carbon_direct::enabled(e) {
            let s=Box::pin(super::super::exec::carbon_direct::state(ctx,e,head)).await?;
            let native=s.native_side.and_then(|i|s.tokens.get(i)).copied();
            let multi=if native.is_some(){ctx.addr("dex.multicall3")?}else{Address::ZERO};
            return Ok(Some(Self{market:None,state:crate::core::types::PoolState::Carbon(Box::new(s)),wrapped:native.unwrap_or_default(),head,multi}));
        }
        if super::super::quote::ekubo_v2_pricing::enabled(e) {
            let crate::core::types::PoolRef::Singleton{manager,id}=e.pool.reference else{return Ok(None);};
            let state=Box::pin(super::super::pools::ekubo_v2::load(ctx,e.family,manager,id,number)).await?;
            return Ok(Some(Self{market:None,state,wrapped:ctx.addr("dex.native_wrapped")?,head,multi:ctx.addr("dex.multicall3")?}));
        }
        if e.family.variant == super::super::pools::family_variant::Variant::BunnySwap {
            let state =
                super::super::pools::bunnyswap::native_state(ctx, e, &head.selector()).await?;
            return Ok(Some(Self {
                market: None,
                state,
                wrapped: ctx.addr("dex.native_wrapped")?,
                head,
                multi: ctx.addr("dex.multicall3")?,
            }));
        }
        let market = vista_pricing::market(ctx, e, head, Form::OfficialEthIn).await?;
        if !market.router_weth.is_zero() {
            return Ok(None);
        }
        let cash: U256 = super::super::node::rpc::call(
            ctx,
            "eth_getBalance",
            serde_json::json!([wallet, head.selector()]),
        )
        .await?;
        if cash < market.sell_fee {
            return Err(ctx.err(
                crate::gateway::errors::ErrorCode::InsufficientBalance,
                "Для пробы EtherVista кошельку не хватает аванса комиссии ETH",
            ));
        }
        Ok(Some(Self {
            wrapped: market.wrapped,
            state: market.state(),
            market: Some(market),
            head,
            multi: ctx.addr("dex.multicall3")?,
        }))
    }
    pub fn market_state(&self)->&crate::core::types::PoolState{&self.state}
    pub fn state(&self) -> crate::core::types::PoolState {
        self.state.clone()
    }
    pub fn leg(
        &self,
        ctx: &Ctx,
        e: &Entry,
        o: &Order,
        wallet: Address,
        now: u64,
    ) -> GatewayResult<(Vec<Approval>, Address, Bytes)> {
        if super::super::quote::ekubo_v2_pricing::enabled(e){return super::super::exec::ekubo_v2_direct::probe(ctx,e,o,now);}
        let eth_in = o.token_in == self.wrapped;
        let form = if eth_in {
            Form::OfficialEthIn
        } else {
            Form::OfficialEthOut
        };
        let (router, call) = match &self.market {
            Some(m) => vista_pricing::wire(ctx, e, o.token_in, m, form)?,
            None => vista_pricing::rabbit_wire(ctx, e, o.token_in)?,
        };
        let approvals = if eth_in {
            Vec::new()
        } else {
            approve::fork_all(ctx, o.token_in, router, o.amount_in)?
        };
        let deadline = U256::from(now.saturating_add(ctx.int("dex.deadline_s")?));
        Ok((
            approvals,
            router,
            call.data(
                o.token_in,
                o.token_out,
                o.amount_in,
                o.min_out,
                wallet,
                deadline,
            ),
        ))
    }
    pub fn value(&self, input: Address, amount: U256) -> U256 {
        let eth_in = input == self.wrapped;
        VistaCall {
            form: if eth_in {
                Form::OfficialEthIn
            } else {
                Form::OfficialEthOut
            },
            wrapped: self.wrapped,
            fee: self
                .market
                .as_ref()
                .map_or(U256::ZERO, |m| if eth_in { m.buy_fee } else { m.sell_fee }),
        }
        .value(amount)
    }
    pub fn fund(&self, wallet: Address, token: Address, amount: U256) -> Option<Call> {
        if matches!(self.state,crate::core::types::PoolState::Carbon(_)){return None;}
        (token == self.wrapped).then(|| {
            pc::call(
                wallet,
                token,
                IProbeAsset::withdrawCall { amount }.abi_encode(),
            )
        })
    }
    pub fn balance(&self, wallet: Address, token: Address) -> Option<Call> {
        (token == self.wrapped).then(|| {
            pc::call(
                wallet,
                self.multi,
                IProbeAsset::getEthBalanceCall { owner: wallet }.abi_encode(),
            )
        })
    }
    pub fn wrap_for_transfer(&self, wallet: Address, token: Address) -> Option<Call> {
        (token == self.wrapped)
            .then(|| pc::call(wallet, token, IProbeAsset::depositCall {}.abi_encode()))
    }
}
