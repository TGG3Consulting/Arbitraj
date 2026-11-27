//! Реестр Integral SIZE, реализация Relayer и доступность немедленной выплаты на одном блоке.
//! https://github.com/IntegralHQ/Integral-SIZE-Smart-Contracts
use super::{
    families::Family,
    nami_batch as batch, solidly_classic_factory as code,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256, U256};
use serde_json::Value;

alloy::sol! { interface ISize {
    function getPair(address a,address b) external view returns(address);
    function allPairsLength() external view returns(uint256);
    function allPairs(uint256 i) external view returns(address);
    function token0() external view returns(address);
    function token1() external view returns(address);
    function trader() external view returns(address);
    function factory() external view returns(address);
    function relayer() external view returns(address);
    function implementation() external view returns(address);
    function delay() external view returns(address);
    function weth() external view returns(address);
    function isPairEnabled(address pair) external view returns(bool);
    function quoteSell(address a,address b,uint256 amount) external view returns(uint256);
    function quoteBuy(address a,address b,uint256 amount) external view returns(uint256);
    function gasPrice() external view returns(uint256);
    function executionGasLimit() external view returns(uint256);
}}
pub(crate) fn bad(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Integral SIZE: не совпали реестр, роли, код или доступность немедленного обмена",
    )
}
#[derive(Clone, Copy)]
pub(crate) struct Roles {
    pub factory: Address,
    pub relay: Address,
    pub implementation: Address,
    pub delay: Address,
    factory_hash: B256,
    relay_hash: B256,
    impl_hash: B256,
    delay_hash: B256,
}
impl Roles {
    pub(crate) fn code_identity(self) -> crate::venues::bsc_dex::exec::carbon_binding::Code {
        crate::venues::bsc_dex::exec::carbon_binding::Code {
            shell: self.relay_hash,
            implementation: self.implementation,
            runtime: self.impl_hash,
        }
    }
    pub(crate) fn read(ctx: &Ctx, id: &str) -> GatewayResult<Self> {
        let scalar = |key| super::safeswap_trade::hash(ctx, id, key);
        let one = |key| -> GatewayResult<String> {
            let rows = ctx.dex_list(key, id)?;
            let [row] = rows.as_slice() else {
                return Err(bad(ctx));
            };
            Ok(row.clone())
        };
        let delay = one("dex.integral_delay")?;
        let fields: Vec<_> = delay.split_whitespace().collect();
        let [a, h] = fields.as_slice() else {
            return Err(bad(ctx));
        };
        let roles = Self {
            factory: ctx.dex_addr("dex.factory", id)?,
            relay: ctx.dex_addr("dex.quoter", id)?,
            implementation: one("dex.proxy_implementations")?
                .parse()
                .map_err(|_| bad(ctx))?,
            delay: a.parse().map_err(|_| bad(ctx))?,
            factory_hash: scalar("dex.factory_runtime_hash")?,
            relay_hash: scalar("dex.quoter_runtime_hash")?,
            impl_hash: one("dex.proxy_runtime_hashes")?
                .parse()
                .map_err(|_| bad(ctx))?,
            delay_hash: h.parse().map_err(|_| bad(ctx))?,
        };
        if [
            roles.factory,
            roles.relay,
            roles.implementation,
            roles.delay,
        ]
        .iter()
        .any(|a| a.is_zero())
            || [roles.impl_hash, roles.delay_hash]
                .iter()
                .any(|h| h.is_zero())
        {
            return Err(bad(ctx));
        }
        Ok(roles)
    }
    pub(super) async fn factory_code(self, ctx: &Ctx, at: &Value) -> GatewayResult<()> {
        code::plain(ctx, self.factory, self.factory_hash, at).await
    }
    async fn code(self, ctx: &Ctx, at: &Value) -> GatewayResult<()> {
        tokio::try_join!(
            code::plain(ctx, self.factory, self.factory_hash, at),
            code::bind(
                ctx,
                self.relay,
                self.relay_hash,
                Some((self.implementation, self.impl_hash)),
                at
            ),
            code::plain(ctx, self.delay, self.delay_hash, at)
        )?;
        Ok(())
    }
}
/// Отключённый разрешённый рынок остаётся в реестре и перечитывается на следующей голове.
pub(crate) async fn state(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    (a, b): (Address, Address),
    at: &Value,
) -> GatewayResult<(Roles, bool)> {
    let g = ctx.settings.generation();
    let r = Roles::read(ctx, f.id)?;
    if a.is_zero() || a >= b || pool.is_zero() {
        return Err(bad(ctx));
    }
    let calls = [
        batch::view(at, r.factory, ISize::getPairCall { a, b }),
        batch::view(at, pool, ISize::token0Call {}),
        batch::view(at, pool, ISize::token1Call {}),
        batch::view(at, pool, ISize::factoryCall {}),
        batch::view(at, pool, ISize::traderCall {}),
        batch::view(at, r.relay, ISize::implementationCall {}),
        batch::view(at, r.relay, ISize::factoryCall {}),
        batch::view(at, r.relay, ISize::delayCall {}),
        batch::view(at, r.delay, ISize::factoryCall {}),
        batch::view(at, r.delay, ISize::relayerCall {}),
        batch::view(at, r.relay, ISize::wethCall {}),
        batch::view(at, r.relay, ISize::isPairEnabledCall { pair: pool }),
    ];
    let (rows, ()) = tokio::try_join!(batch::read(ctx, calls), r.code(ctx, at))?;
    for (row, expected) in rows.iter().zip([
        pool,
        a,
        b,
        r.factory,
        r.delay,
        r.implementation,
        r.factory,
        r.delay,
        r.factory,
        r.relay,
        ctx.addr("dex.native_wrapped")?,
    ]) {
        if batch::decode(row, ISize::factoryCall {})? != expected {
            return Err(bad(ctx));
        }
    }
    let enabled = batch::decode(&rows[11], ISize::isPairEnabledCall { pair: pool })?;
    if ctx.settings.generation() != g {
        return Err(bad(ctx));
    }
    Ok((r, enabled))
}
pub(crate) async fn module(ctx: &Ctx, f: &Family, r: Roles, at: &Value) -> GatewayResult<Address> {
    let g = ctx.settings.generation();
    let m = ctx.dex_addr("dex.router", f.id)?;
    let (rows, ()) = tokio::try_join!(
        batch::read(
            ctx,
            [
                batch::view(at, m, ISize::factoryCall {}),
                batch::view(at, m, ISize::delayCall {}),
                batch::view(at, m, ISize::relayerCall {}),
                batch::view(at, m, ISize::implementationCall {})
            ]
        ),
        super::direct_module::verify_code(ctx, f.id, m, at)
    )?;
    for (row, expected) in rows
        .iter()
        .zip([r.factory, r.delay, r.relay, r.implementation])
    {
        if batch::decode(row, ISize::factoryCall {})? != expected {
            return Err(bad(ctx));
        }
    }
    if ctx.settings.generation() != g {
        return Err(bad(ctx));
    }
    Ok(m)
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let check: GatewayResult<PoolRef> = async {
        let pool = c.pool.parse().map_err(|_| bad(ctx))?;
        let at = super::nami::block(ctx, "latest").await?;
        state(
            ctx,
            c.family,
            pool,
            super::discover::sorted(c.coin, c.quote),
            &at,
        )
        .await?;
        Ok(PoolRef::Contract(pool))
    }
    .await;
    match check {
        Ok(p) => Verdict::Taken(Box::new(super::verify_woo::found(c, p))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) async fn funded(ctx: &Ctx, r: Roles, at: &Value) -> GatewayResult<bool> {
    use crate::venues::bsc_dex::node::rpc;
    let (rows, balance): (_, U256) = tokio::try_join!(
        batch::read(
            ctx,
            [
                batch::view(at, r.delay, ISize::gasPriceCall {}),
                batch::view(at, r.relay, ISize::executionGasLimitCall {})
            ]
        ),
        rpc::call(ctx, "eth_getBalance", serde_json::json!([r.relay, at]))
    )?;
    let price = batch::decode(&rows[0], ISize::gasPriceCall {})?;
    let limit = batch::decode(&rows[1], ISize::executionGasLimitCall {})?;
    Ok(price.checked_mul(limit).is_some_and(|fee| fee <= balance))
}
