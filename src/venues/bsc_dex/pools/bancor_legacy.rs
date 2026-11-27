//! Bancor 2.1. Роли, владелец anchor, физические резервы и котировка читаются на одном блоке.
use super::{families::Family, nami_batch as batch, solidly_classic_factory::plain};
use crate::venues::bsc_dex::node::{abi::IERC20, rpc};
use crate::{
    core::types::bancor_legacy::BancorLegacyState,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, U256};
use serde_json::{Value, json};

sol! {
    interface ILegacy {
        function registry() external view returns(address);
        function addressOf(bytes32 name) external view returns(address);
        function getLiquidityPools() external view returns(address[]);
        function getConvertersBySmartTokens(address[] anchors) external view returns(address[]);
        function isLiquidityPool(address anchor) external view returns(bool);
        function getConvertibleTokenSmartTokens(address reserve) external view returns(address[]);
        function owner() external view returns(address);
        function token() external view returns(address);
        function connectorTokenCount() external view returns(uint16);
        function connectorTokens(uint256 i) external view returns(address);
        function getConnectorBalance(address asset) external view returns(uint256);
        function conversionFee() external view returns(uint32);
        function rateByPath(address[] path,uint256 amount) external view returns(uint256);
        event LiquidityPoolAdded(address indexed liquidityPool);
        event Conversion(address indexed sourceToken,address indexed targetToken,address indexed trader,uint256 sourceAmount,uint256 targetAmount,int256 conversionFee);
    }
}
pub(super) fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Bancor 2.1: не подтверждены реестр, конвертер или денежная пара",
    )
}
pub(super) async fn read<C: SolCall + Clone>(
    ctx: &Ctx,
    addr: Address,
    q: C,
    at: &Value,
) -> GatewayResult<C::Return> {
    let [out] = batch::read(ctx, [batch::view(at, addr, q.clone())]).await?;
    batch::decode(&out, q)
}
pub(super) struct Roles {
    pub registry: Address,
    pub network: Address,
    pub native: Address,
    pub wrapped: Address,
}
fn name(s: &[u8]) -> B256 {
    let mut b = [0u8; 32];
    b[..s.len()].copy_from_slice(s);
    b.into()
}
impl Roles {
    pub(super) async fn read(ctx: &Ctx, f: &Family, at: &Value) -> GatewayResult<Self> {
        let registry = ctx.dex_addr("dex.factory", f.id)?;
        let network = ctx.dex_addr("dex.quoter", f.id)?;
        let factory_hash = super::safeswap_trade::hash(ctx, f.id, "dex.factory_runtime_hash")?;
        let network_hash = super::safeswap_trade::hash(ctx, f.id, "dex.quoter_runtime_hash")?;
        let (_, _, registry_root) = tokio::try_join!(
            plain(ctx, registry, factory_hash, at),
            plain(ctx, network, network_hash, at),
            read(ctx, registry, ILegacy::registryCall {}, at)
        )?;
        let (r, n) = tokio::try_join!(
            read(
                ctx,
                registry_root,
                ILegacy::addressOfCall {
                    name: name(b"BancorConverterRegistry")
                },
                at
            ),
            read(
                ctx,
                registry_root,
                ILegacy::addressOfCall {
                    name: name(b"BancorNetwork")
                },
                at
            )
        )?;
        let native = ctx.dex_addr("dex.bancor_native_token", f.id)?;
        let wrapped = ctx.addr("dex.native_wrapped")?;
        if r != registry || n != network || native == wrapped {
            return Err(bad(ctx));
        }
        Ok(Self {
            registry,
            network,
            native,
            wrapped,
        })
    }
    pub(super) fn normalize(&self, a: Address) -> Address {
        if a == self.native { self.wrapped } else { a }
    }
}
async fn held(
    ctx: &Ctx,
    contract: Address,
    asset: Address,
    native: Address,
    at: &Value,
) -> GatewayResult<U256> {
    if asset == native {
        rpc::call(ctx, "eth_getBalance", json!([contract, at])).await
    } else {
        read(ctx, asset, IERC20::balanceOfCall { owner: contract }, at).await
    }
}
pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    converter: Address,
    expected: (Address, Address),
    at: &Value,
) -> GatewayResult<BancorLegacyState> {
    let r = Roles::read(ctx, f, at).await?;
    let (anchor, count, first, second, fee_ppm) = tokio::try_join!(
        read(ctx, converter, ILegacy::tokenCall {}, at),
        read(ctx, converter, ILegacy::connectorTokenCountCall {}, at),
        read(
            ctx,
            converter,
            ILegacy::connectorTokensCall { i: U256::ZERO },
            at
        ),
        read(
            ctx,
            converter,
            ILegacy::connectorTokensCall { i: U256::ONE },
            at
        ),
        read(ctx, converter, ILegacy::conversionFeeCall {}, at)
    )?;
    let mut reserves = [first, second];
    if r.normalize(first) > r.normalize(second) {
        reserves.swap(0, 1)
    }
    let tokens = reserves.map(|a| r.normalize(a));
    if count != 2
        || anchor.is_zero()
        || first.is_zero()
        || second.is_zero()
        || tokens[0] == tokens[1]
        || tokens != [expected.0, expected.1]
        || fee_ppm > 1_000_000
    {
        return Err(bad(ctx));
    }
    let (owner, liquid, assigned, b0, b1) = tokio::try_join!(
        read(ctx, anchor, ILegacy::ownerCall {}, at),
        read(ctx, r.registry, ILegacy::isLiquidityPoolCall { anchor }, at),
        read(
            ctx,
            r.registry,
            ILegacy::getConvertersBySmartTokensCall {
                anchors: vec![anchor]
            },
            at
        ),
        read(
            ctx,
            converter,
            ILegacy::getConnectorBalanceCall { asset: reserves[0] },
            at
        ),
        read(
            ctx,
            converter,
            ILegacy::getConnectorBalanceCall { asset: reserves[1] },
            at
        )
    )?;
    if owner != converter || !liquid || assigned != [converter] {
        return Err(bad(ctx));
    }
    let (h0, h1) = tokio::try_join!(
        held(ctx, converter, reserves[0], r.native, at),
        held(ctx, converter, reserves[1], r.native, at)
    )?;
    Ok(BancorLegacyState {
        wallet_balances: [h0, h1],
        anchor,
        network: r.network,
        converter,
        tokens,
        reserves,
        fee_ppm,
        balances: [b0, b1],
        levels: Default::default(),
    })
}
