//! Поручение Omni Flux с физической оплатой ETH через опубликованный Universal Router.
//! SETTLE погашает точное количество. TAKE_ALL возвращает кредит исходному кошельку.
use super::super::{
    ctx::Ctx,
    pools::{families::Proto, key::PoolKey, registry::Entry},
};
use super::{
    swap_v4::{InfKey, InfSingle},
    ur,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, Bytes, U256, aliases::U24};
#[path = "flux_eth_receipt.rs"]
mod receipt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Market {
    pub router: Address,
    pub permit: Address,
    pub wrapped: Address,
    pub token: Address,
    pub manager: Address,
    pub id: B256,
    pub bin: bool,
    pub fee: u32,
    pub parameters: B256,
}
impl Market {
    pub(in crate::venues::bsc_dex) fn read(ctx: &Ctx, e: &Entry) -> GatewayResult<Self> {
        let bad = || {
            ctx.err(
                ErrorCode::NoRoute,
                "Omni Flux: нет подтверждённого нативного рынка",
            )
        };
        let Some(PoolKey::Infinity(k)) = &e.key else {
            return Err(bad());
        };
        let crate::core::types::PoolRef::Singleton { manager, id } = e.pool.reference else {
            return Err(bad());
        };
        let wrapped = ctx.addr("dex.native_wrapped")?;
        if !e.family.variant.flux_native()
            || wrapped != e.pool.token0
            || k.manager != manager
            || !k.currency0.is_zero()
            || k.currency1 != e.pool.token1
            || k.currency1 == wrapped
            || !k.hooks.is_zero()
            || super::super::pools::key::infinity_hook_bits(k.parameters) != 0
            || e.key.as_ref().is_none_or(|p| p.id() != id)
        {
            return Err(bad());
        }
        Ok(Self {
            router: ctx.dex_addr("dex.universal_router", e.family.id)?,
            permit: ctx.dex_addr("dex.permit2", e.family.id)?,
            wrapped,
            token: k.currency1,
            manager,
            id,
            bin: e.family.proto == Proto::InfinityBin,
            fee: k.fee,
            parameters: k.parameters,
        })
    }
    pub(in crate::venues::bsc_dex) fn proof(
        self,
        o: &super::super::quote::v2_wallet_sim::Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        Proof {
            market: self,
            payer: o.payer,
            input: o.input,
            output: o.output,
            amount: o.amount,
            minimum,
            deadline,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    pub market: Market,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Proof {
    pub(in crate::venues::bsc_dex) fn value(&self) -> U256 {
        if self.input == self.market.wrapped {
            self.amount
        } else {
            U256::ZERO
        }
    }
    pub(in crate::venues::bsc_dex) fn data(&self) -> Bytes {
        let m = self.market;
        let buy = self.input == m.wrapped;
        let Ok(amount) = u128::try_from(self.amount) else {
            return Bytes::new();
        };
        let Ok(minimum) = u128::try_from(self.minimum) else {
            return Bytes::new();
        };
        let key = InfKey {
            currency0: Address::ZERO,
            currency1: m.token,
            hooks: Address::ZERO,
            poolManager: m.manager,
            fee: U24::from(m.fee),
            parameters: m.parameters,
        };
        let swap = InfSingle {
            poolKey: key,
            zeroForOne: buy,
            amountIn: amount,
            amountOutMinimum: minimum,
            hookData: Bytes::new(),
        }
        .abi_encode();
        let (tin, tout) = if buy {
            (Address::ZERO, m.token)
        } else {
            (m.token, Address::ZERO)
        };
        let actions = Bytes::from(vec![if m.bin { 0x1c } else { 0x06 }, 0x0b, 0x0f]);
        let params = vec![
            Bytes::from(swap),
            Bytes::from((tin, self.amount, true).abi_encode_params()),
            Bytes::from((tout, self.minimum).abi_encode_params()),
        ];
        ur::execute(
            vec![(ur::V4_SWAP, (actions, params).abi_encode_params().into())],
            self.deadline,
        )
    }
}
