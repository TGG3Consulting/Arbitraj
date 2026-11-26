//! Supernova CL. Читаем скидку отправителя и результат beforeSwap в каноническом состоянии.
//! Вызов от пула выполняет запись оракула только внутри eth_call. Квотер получает реальный tx.origin.
use super::{
    abi_algebra_integral::IAlgebraIntegralPool as Pool, families::Family, nami_batch as batch,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc, wallet::signer},
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, keccak256};
use serde_json::{Value, json};

sol! {
    interface Supernova {
        function factory() external view returns(address);
        function poolDeployer() external view returns(address);
        function defaultPluginFactory() external view returns(address);
        function algebraFactory() external view returns(address);
        function token0() external view returns(address);
        function token1() external view returns(address);
        function poolByPair(address a,address b) external view returns(address);
        function pluginByPool(address p) external view returns(address);
        function plugin() external view returns(address);
        function pool() external view returns(address);
        function getRouter() external view returns(address);
        function feeDiscountRegistry() external view returns(address);
        function getSecurityRegistry() external view returns(address);
        function feeDiscounts(address user,address market) external view returns(uint16);
        function beforeSwap(address sender,address recipient,bool direction,int256 amount,uint160 limit,bool prepaid,bytes data) external returns(bytes4 response,uint24 overrideFee,uint24 pluginFee);
    }
}

macro_rules! views {
    ($ctx:expr,$tag:expr; $($v:ident: $to:expr => $call:expr),+) => {
        let [$($v),+] = batch::read($ctx,[$(batch::view($tag,$to,$call)),+]).await?;
        $(let $v=batch::decode(&$v,$call)?;)+
    }
}

// Девять immutable ссылок на pool в этой версии. Хеш нормализует только операнды PUSH32 с его адресом.
fn template(code: &[u8], pool: Address) -> Option<B256> {
    let mut normalized = code.to_vec();
    let word = pool.into_word();
    let positions: Vec<_> = code
        .windows(33)
        .enumerate()
        .filter(|(_, instruction)| instruction[0] == 0x7f && instruction[1..] == word[..])
        .map(|(i, _)| i + 1)
        .collect();
    if positions.is_empty() {
        return None;
    }
    for i in positions {
        normalized[i..i + 32].fill(0);
    }
    Some(keccak256(normalized))
}

// Контракт округляет floor(fee * (1000 - discount) / 1000). При ненулевой скидке у самого пула
// восстанавливаем лишь однозначный результат для действующего кошелька.
fn projected(observed: u32, reference: u16, actual: u16) -> Option<u16> {
    if reference > 1000 || actual > 1000 {
        return None;
    }
    let b = 1000 - u32::from(actual);
    if b == 0 {
        return Some(0);
    }
    if reference == actual {
        return u16::try_from(observed).ok();
    }
    let a = 1000 - u32::from(reference);
    if a == 0 {
        return None;
    }
    let low = (observed * 1000).div_ceil(a);
    if low > u32::from(u16::MAX) {
        return None;
    }
    let high = ((observed + 1) * 1000).div_ceil(a) - 1;
    let (low, high) = (low * b / 1000, high.min(u32::from(u16::MAX)) * b / 1000);
    if low != high {
        return None;
    }
    u16::try_from(low).ok()
}

pub(in crate::venues::bsc_dex) async fn check(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tag: &Value,
) -> GatewayResult<(u16, u8)> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Supernova: привязка рынка, код плагина, скидка или флаги изменились",
        )
    };
    let generation = ctx.settings.generation();
    let factory = super::solidly_classic_factory::at(ctx, f, tag).await?;
    let registry = ctx.dex_addr("dex.plugin_factory", f.id)?;
    let quoter = ctx.dex_addr("dex.quoter", f.id)?;
    let hashes = ctx
        .dex_list("dex.plugin_runtime_hashes", f.id)?
        .iter()
        .map(|v| v.parse::<B256>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| bad())?;
    let [
        registry_hash,
        plugin_hash,
        quoter_hash,
        security_hash,
        discount_hash,
    ] = hashes.as_slice()
    else {
        return Err(bad());
    };
    tokio::try_join!(
        super::solidly_classic_factory::plain(ctx, registry, *registry_hash, tag),
        super::solidly_classic_factory::plain(ctx, quoter, *quoter_hash, tag)
    )?;
    let init = super::verify_algebra_dir::hashes_for(ctx, f.id).map_err(|_| bad())?;
    views!(ctx,tag; factory_of:pool=>Supernova::factoryCall{}, linked:factory=>Supernova::defaultPluginFactoryCall{},
        back:registry=>Supernova::algebraFactoryCall{}, deployer:factory=>Supernova::poolDeployerCall{},
        q_factory:quoter=>Supernova::factoryCall{}, q_deployer:quoter=>Supernova::poolDeployerCall{},
        a:pool=>Supernova::token0Call{}, b:pool=>Supernova::token1Call{}, plugin:pool=>Supernova::pluginCall{},
        registered:registry=>Supernova::pluginByPoolCall{p:pool},gs:pool=>Pool::globalStateCall{});
    if (factory_of, linked, back, q_factory, q_deployer)
        != (factory, registry, factory, factory, deployer)
        || plugin.is_zero()
        || plugin != registered
        || a.is_zero()
        || a >= b
        || !gs.unlocked
        || !matches!(gs.pluginConfig, 87 | 215)
        || !init
            .iter()
            .any(|h| super::verify_algebra_dir::pool_address(deployer, a, b, *h) == pool)
    {
        return Err(bad());
    }
    views!(ctx,tag; pair:factory=>Supernova::poolByPairCall{a,b}, owner:plugin=>Supernova::poolCall{},
        reflex:plugin=>Supernova::getRouterCall{}, security:plugin=>Supernova::getSecurityRegistryCall{},
        discount:plugin=>Supernova::feeDiscountRegistryCall{});
    let origin = signer::address(ctx)?;
    let sender = ctx.executor()?.unwrap_or(quoter);
    if pair != pool || owner != pool || sender == reflex || quoter == reflex {
        return Err(bad());
    }
    let (code, _, _) = tokio::try_join!(
        rpc::call::<Bytes>(ctx, "eth_getCode", json!([plugin, tag])),
        super::solidly_classic_factory::plain(ctx, security, *security_hash, tag),
        super::solidly_classic_factory::plain(ctx, discount, *discount_hash, tag)
    )?;
    if template(&code, pool) != Some(*plugin_hash) {
        return Err(bad());
    }
    let probe = Supernova::beforeSwapCall {
        sender,
        recipient: origin,
        direction: true,
        amount: Default::default(),
        limit: Default::default(),
        prepaid: false,
        data: Bytes::new(),
    };
    let [before, reference, actual] = batch::read(
        ctx,
        [
            (
                "eth_call",
                json!([{"from":pool,"to":plugin,"data":Bytes::from(probe.abi_encode())},tag]),
            ),
            batch::view(
                tag,
                discount,
                Supernova::feeDiscountsCall {
                    user: pool,
                    market: pool,
                },
            ),
            batch::view(
                tag,
                discount,
                Supernova::feeDiscountsCall {
                    user: origin,
                    market: pool,
                },
            ),
        ],
    )
    .await?;
    let result = batch::decode(&before, probe)?;
    let discount_call = Supernova::feeDiscountsCall {
        user: origin,
        market: pool,
    };
    let reference = batch::decode(&reference, discount_call.clone())?;
    let actual = batch::decode(&actual, discount_call)?;
    let observed = result.overrideFee.to::<u32>();
    if result.response.as_slice() != Supernova::beforeSwapCall::SELECTOR
        || !result.pluginFee.is_zero()
        || observed > u32::from(u16::MAX)
    {
        return Err(bad());
    }
    let override_fee = projected(observed, reference, actual).ok_or_else(bad)?;
    if (gs.pluginConfig == 87 && override_fee != 0) || generation != ctx.settings.generation() {
        return Err(bad());
    }
    Ok((
        if override_fee == 0 {
            gs.lastFee
        } else {
            override_fee
        },
        gs.pluginConfig,
    ))
}

pub(in crate::venues::bsc_dex) async fn quote(
    ctx: &Ctx,
    id: &str,
    input: Address,
    output: Address,
    amount: alloy_primitives::U256,
    at: &str,
) -> GatewayResult<alloy_primitives::U256> {
    let hash = super::elfomo::block(ctx, at).await?;
    let payer = signer::address(ctx)?;
    let target = ctx.dex_addr("dex.quoter", id)?;
    let data = super::nami::calldata(input, output, amount);
    let raw: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"from":payer,"to":target,"data":data},hash]),
    )
    .await?;
    super::nami::decoded(&raw)
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "Supernova: ответ квотера не по ABI"))
}
