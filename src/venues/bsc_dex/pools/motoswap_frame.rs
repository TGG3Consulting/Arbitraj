//! Общие для пар Motoswap роли, комиссии и версия модуля на каноническом блоке.
//! Загрузчик состояния пары вызывает read и path_module. Они проверяют контракты через
//! EIP-1898 и используют кэш этой площадки и области DEX из motoswap_cache.rs.
//! Адреса и тарифы FeeRouter: https://docs.motoswap.org/dev/guides/swap-via-fee-router
use super::{Address, B256, Ctx, Family, GatewayResult, IMoto, Pair, Role, U256, Value, bad, batch};

#[path = "motoswap_cache.rs"]
mod cache;
pub(crate) use cache::Book;

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub(super) struct Roles {
    pub factory: Role,
    pub fee: Role,
    pub core: Role,
    pub beacon: Role,
    pub pair_impl: Role,
    pub registry: Role,
    pub creators: Role,
    pub vault: Role,
    pub collector: Role,
    pub wrapped: Address,
    pub init: B256,
}
#[derive(Clone, Copy)]
pub(super) struct Frame {
    pub lp: u16,
    pub protocol: u16,
    pub creator: u16,
    pub frozen: bool,
}

/// Принимает роли профиля и канонический селектор блока. Возвращает проверенные комиссии
/// и признак неизменности роутера. Ошибки селектора, RPC и проверки ролей передаются загрузчику.
pub(super) async fn read(ctx: &Ctx, f: &Family, r: Roles, at: &Value) -> GatewayResult<Frame> {
    let hash = at["blockHash"]
        .as_str()
        .and_then(|h| h.parse::<B256>().ok())
        .filter(|_| at["requireCanonical"] == true)
        .ok_or_else(|| bad(ctx))?;
    let cell = ctx.evm_chain.motoswap().frame(ctx, f.id, hash, r);
    Ok(*cell.get_or_try_init(|| load(ctx, r, at)).await?)
}
/// Принимает полный набор ролей и блок. Возвращает кадр после сверки кода, связей,
/// разрешений и ставок. При несовпадении или ошибке узла результат не сохраняется.
async fn load(ctx: &Ctx, r: Roles, at: &Value) -> GatewayResult<Frame> {
    let Roles {
        factory,
        fee,
        core,
        beacon,
        pair_impl,
        registry,
        creators,
        vault,
        collector,
        wrapped,
        init,
    } = r;
    tokio::try_join!(
        factory.check(ctx, at),
        fee.check(ctx, at),
        core.check(ctx, at),
        beacon.check(ctx, at),
        pair_impl.check(ctx, at),
        registry.check(ctx, at),
        creators.check(ctx, at),
        vault.check(ctx, at),
        collector.check(ctx, at)
    )?;
    let fac = factory.address;
    let router = fee.address;
    let allowed = |who| batch::view(at, fac, IMoto::swapAllowedCall { who });
    let [
        core_factory,
        core_allowed,
        fee_allowed,
        fee_weth,
        core_weth,
        official_core,
        quote_registry,
        factory_registry,
        creator_registry,
        creator_vault,
        col,
        fac_beacon,
        beacon_impl,
        factory_impl,
        init_hash,
        lp,
        protocol,
        creator,
        frozen,
    ] = batch::read(
        ctx,
        [
            batch::view(at, core.address, Pair::factoryCall {}),
            allowed(core.address),
            allowed(router),
            batch::view(at, router, IMoto::WETHCall {}),
            batch::view(at, core.address, IMoto::WETHCall {}),
            batch::view(at, router, IMoto::routerCall {}),
            batch::view(at, router, IMoto::registryCall {}),
            batch::view(at, fac, IMoto::quoteRegistryCall {}),
            batch::view(at, router, IMoto::creatorRegistryCall {}),
            batch::view(at, router, IMoto::creatorVaultCall {}),
            batch::view(at, router, IMoto::collectorCall {}),
            batch::view(at, fac, IMoto::beaconCall {}),
            batch::view(at, beacon.address, IMoto::implementationCall {}),
            batch::view(at, fac, IMoto::implementationCall {}),
            batch::view(at, fac, IMoto::pairCodeHashCall {}),
            batch::view(at, fac, IMoto::swapFeeBpsCall {}),
            batch::view(at, router, IMoto::protocolFeeBpsCall {}),
            batch::view(at, router, IMoto::creatorFeeBpsCall {}),
            batch::view(at, router, IMoto::upgradesRenouncedCall {}),
        ],
    )
    .await?;
    if [
        (core_factory, fac),
        (fee_weth, wrapped),
        (core_weth, wrapped),
        (official_core, core.address),
        (quote_registry, registry.address),
        (factory_registry, registry.address),
        (creator_registry, creators.address),
        (creator_vault, vault.address),
        (col, collector.address),
        (fac_beacon, beacon.address),
        (beacon_impl, pair_impl.address),
        (factory_impl, pair_impl.address),
    ]
    .iter()
    .any(|(raw, want)| raw.as_ref() != want.into_word().as_slice())
        || init_hash.as_ref() != init.as_slice()
        || !batch::decode(&core_allowed, IMoto::swapAllowedCall { who: core.address })?
        || !batch::decode(&fee_allowed, IMoto::swapAllowedCall { who: router })?
    {
        return Err(bad(ctx));
    }
    let rate = |n: U256| u16::try_from(n).map_err(|_| bad(ctx));
    Ok(Frame {
        lp: rate(batch::decode(&lp, IMoto::swapFeeBpsCall {})?)?,
        protocol: rate(batch::decode(&protocol, IMoto::protocolFeeBpsCall {})?)?,
        creator: rate(batch::decode(&creator, IMoto::creatorFeeBpsCall {})?)?,
        frozen: batch::decode(&frozen, IMoto::upgradesRenouncedCall {})?,
    })
}

/// Принимает модуль, профиль и блок. Возвращает адрес при подтверждённом коде и pathVersion.
/// При ошибке возвращает None: обычный обмен через одну пару остаётся доступным.
pub(super) async fn path_module(
    ctx: &Ctx,
    f: &super::Family,
    module: Address,
    at: &Value,
) -> Option<Address> {
    let hash = at["blockHash"].as_str()?.parse::<B256>().ok()?;
    let allowed = crate::venues::bsc_dex::pools::direct_module::configured(ctx, f.id).ok()?;
    let cell = ctx.evm_chain.motoswap().version(ctx, f.id, hash, module, allowed);
    let supported = cell
        .get_or_try_init(|| async {
            crate::venues::bsc_dex::pools::direct_module::verified_hash(ctx, f.id, module, at)
                .await?;
            crate::venues::bsc_dex::node::call::canonical(
                ctx,
                at,
                module,
                IMoto::pathVersionCall {},
            )
            .await
            .map(|v| v == U256::ONE)
        })
        .await
        .ok()?;
    supported.then_some(module)
}
