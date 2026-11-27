//! Идентичность размещения Fermi в настройках площадки Ethereum.
use super::{Ctx, canonical};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy_primitives::{Address, B256};
alloy::sol! {
    function ROUTER() external view returns(address);
    function ENGINE() external view returns(address);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Deployment {
    pub router: Address,
    pub engine: Address,
    pub runtime: B256,
}
impl Deployment {
    pub fn read(ctx: &Ctx) -> GatewayResult<Self> {
        let generation = ctx.settings.generation();
        let router = ctx.addr("fermi.router")?;
        let engine = ctx.addr("fermi.engine")?;
        let runtime = runtime(ctx, "fermi.router_runtime_hash")?;
        if router.is_zero() || engine.is_zero() || engine == router {
            return Err(ctx.err(
                ErrorCode::InvalidAddress,
                "Fermi: нужны разные адреса роутера и движка",
            ));
        }
        if ctx.settings.generation() != generation {
            return Err(ctx.err(
                ErrorCode::StaleData,
                "Fermi: сменились настройки размещения",
            ));
        }
        Ok(Self {
            router,
            engine,
            runtime,
        })
    }
}
pub(super) fn runtime(ctx: &Ctx, key: &str) -> GatewayResult<B256> {
    ctx.text(key)?
        .trim()
        .parse::<B256>()
        .ok()
        .filter(|v| !v.is_zero())
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NotApplicable,
                format!("Fermi: нужен 32-байтный хеш кода {key}"),
            )
        })
}
pub(super) async fn executor_router(
    ctx: &Ctx,
    block: B256,
    executor: Address,
) -> GatewayResult<Address> {
    canonical(ctx, block, executor, ROUTERCall {}).await
}
pub(super) async fn executor_engine(
    ctx: &Ctx,
    block: B256,
    executor: Address,
) -> GatewayResult<Address> {
    canonical(ctx, block, executor, ENGINECall {}).await
}
