//! 0x публикует текущий и предыдущий Settler для функции 2 в реестре размещений.
//! https://github.com/0xProject/0x-settler
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::call, pools::solidly_classic_factory},
};
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;
alloy::sol! { interface IRegistry {function ownerOf(uint256 id) external view returns(address);function prev(uint128 feature) external view returns(address);}}
pub(super) async fn verify(ctx: &Ctx, settler: Address, at: &Value) -> GatewayResult<()> {
    let fail = || super::bad(ctx, "Реестр 0x не подтвердил действующий Settler");
    let data = ctx.text("zeroex.settler_registry")?;
    let fields = data.split_whitespace().collect::<Vec<_>>();
    let [proxy, shell, logic, code] = fields.as_slice() else {
        return Err(fail());
    };
    let address = |s: &str| {
        s.parse::<Address>()
            .ok()
            .filter(|x| !x.is_zero())
            .ok_or_else(fail)
    };
    let hash = |s: &str| {
        s.parse::<B256>()
            .ok()
            .filter(|x| !x.is_zero())
            .ok_or_else(fail)
    };
    let registry = address(proxy)?;
    let ((), current, prev) = tokio::try_join!(
        solidly_classic_factory::bind(
            ctx,
            registry,
            hash(shell)?,
            Some((address(logic)?, hash(code)?)),
            at
        ),
        call::canonical(
            ctx,
            at,
            registry,
            IRegistry::ownerOfCall { id: U256::from(2) }
        ),
        call::canonical(ctx, at, registry, IRegistry::prevCall { feature: 2 })
    )?;
    if settler != current && settler != prev {
        return Err(fail());
    }
    Ok(())
}
