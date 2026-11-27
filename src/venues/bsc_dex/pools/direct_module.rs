//! Файл: venues/bsc_dex/pools/direct_module.rs.
//! Что делает: связывает адрес рынка и проверенный runtime отдельного исполнителя с протоколом и сетью.
//! Кто вызывает: families, exec и quote. От чего зависит: активные настройки выбранного DEX.
//! Настройки: dex.kind, dex.router и dex.module_runtime_hashes.
use super::super::{ctx::Ctx, node::rpc};
use super::{elfomo, families::Family, registry::Entry};
use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy_primitives::{Address, B256, Bytes, keccak256};
use serde_json::{Value, json};
/// Список совместимых сборок задаётся отдельно для каждого DEX, включая суффикс сети.
pub fn configured(ctx: &Ctx, id: &str) -> GatewayResult<Vec<B256>> {
    let values = ctx.dex_list("dex.module_runtime_hashes", id)?;
    let hashes = values
        .iter()
        .map(|v| v.parse::<B256>())
        .collect::<Result<Vec<_>, _>>();
    let wrong = || {
        let message = format!("dex.module_runtime_hashes [dex:{id}]: нужны хеши допустимых сборок");
        ctx.not_set("dex.module_runtime_hashes", message)
    };
    let hashes = hashes.map_err(|_| wrong())?;
    if hashes.is_empty()
        || hashes
            .iter()
            .any(|h| *h == B256::ZERO || *h == keccak256([]))
    {
        return Err(wrong());
    }
    Ok(hashes)
}

pub fn market(ctx: &Ctx, entry: &Entry) -> GatewayResult<Address> {
    match (&entry.pool.reference, entry.family.proto) {
        (
            PoolRef::Member { contract, .. },
            super::families::Proto::Elfomo | super::families::Proto::BaiBai,
        )
        | (
            PoolRef::Contract(contract),
            super::families::Proto::BaiBai | super::families::Proto::Lunar,
        ) => {
            let p = super::pmm_profile::Profile::read(ctx, entry.family.id, entry.family.proto)?;
            if *contract != p.pool {
                return Err(ctx.err(
                    ErrorCode::NoRoute,
                    "Ссылка рынка расходится с рабочим dex.pmm_pool",
                ));
            }
            Ok(*contract)
        }
        (PoolRef::Contract(address), super::families::Proto::PancakeV3)
            if matches!(entry.family.variant, super::family_variant::Variant::BaseNativeV3 | super::family_variant::Variant::VvsV3) =>
        {
            Ok(*address)
        }
        (PoolRef::Contract(address), super::families::Proto::UniswapV3)
            if entry.family.variant == super::family_variant::Variant::RegisteredV3 => Ok(*address),
        (PoolRef::Contract(address), super::families::Proto::V2)
            if matches!(entry.family.variant,super::family_variant::Variant::Stabull | super::family_variant::Variant::MaverickOne) || entry.family.variant.umbrae() || matches!(entry.family.variant,super::family_variant::Variant::Virtuals | super::family_variant::Variant::EmpireModule | super::family_variant::Variant::CandySwap | super::family_variant::Variant::Memebox | super::family_variant::Variant::BunnySwap) => Ok(*address),
        (PoolRef::Contract(address),super::families::Proto::KyberElastic)
            if entry.family.variant==super::family_variant::Variant::ElasticV5 && entry.family.tradable => Ok(*address),
        (PoolRef::Contract(address), super::families::Proto::Slipstream) => Ok(*address),
        _ => Err(ctx.err(
            ErrorCode::NoRoute,
            "Нет подтверждённого рынка прямого модуля",
        )),
    }
}

/// Подтверждает байты, прочитанные с адреса исполнителя на проверяемом блоке.
pub fn matching_hash(hashes: &[B256], code: &[u8]) -> Option<B256> {
    if code.is_empty() {
        return None;
    }
    let hash = keccak256(code);
    hashes.contains(&hash).then_some(hash)
}

pub async fn verify_code(ctx: &Ctx, id: &str, addr: Address, tag: &Value) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let expected = configured(ctx, id)?;
    if ctx.dex_addr("dex.router", id)? != addr {
        return Err(ctx.err(
            ErrorCode::StaleData,
            format!("DEX {id}: адрес модуля изменился"),
        ));
    }
    let code: Bytes = rpc::call(ctx, "eth_getCode", json!([addr, tag])).await?;
    let Some(hash) = matching_hash(&expected, &code) else {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            format!("DEX {id}: runtime модуля {addr} отсутствует в dex.module_runtime_hashes"),
        ));
    };
    super::pmm_binding::verify(ctx, id, &code, hash)?;
    if generation != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            format!("DEX {id}: настройки изменились во время проверки модуля"),
        ));
    }
    Ok(())
}

/// Вход проверки V5 с уже выбранным каноническим блоком. Прямой кошелёк проходит сюда через verify().
pub async fn verify_factory(ctx: &Ctx, f: &Family, addr: Address, at: &Value) -> GatewayResult<()> {
    let generation=ctx.settings.generation();
    let kind=ctx.dex_text("dex.kind",f.id)?;
    if at["requireCanonical"]!=true || at["blockHash"].as_str().and_then(|s|s.parse::<B256>().ok()).is_none() {
        return Err(ctx.err(ErrorCode::BadResponse,"Для сверки модуля требуется канонический хеш блока"));
    }
    if f.variant == super::family_variant::Variant::BunnySwap {
        if kind!="bunnyswap_module" || !f.tradable { return Err(ctx.err(ErrorCode::NoRoute,"Не включён модуль RabbitRouter")); }
        verify_code(ctx,f.id,addr,at).await?;
        let c=super::bunnyswap::IRabbit::routerCall{};
        let [r]=super::nami_batch::read(ctx,[super::nami_batch::view(at,addr,c.clone())]).await?;
        if super::nami_batch::decode(&r,c)?!=ctx.dex_addr("dex.quoter",f.id)? || generation!=ctx.settings.generation(){
            return Err(ctx.err(ErrorCode::QuoteMismatch,"Модуль связан с другим RabbitRouter или настройки изменились"));
        }
        return Ok(());
    }
    let expected=match f.variant {
        super::family_variant::Variant::Stabull => "stabull_module",
        super::family_variant::Variant::MaverickOne => "maverick_v1_module",
        super::family_variant::Variant::EmpireModule => "empire_module",
        super::family_variant::Variant::CandySwap => "candyswap_module",
        super::family_variant::Variant::Memebox => "memebox_v5",
        super::family_variant::Variant::RegisteredV3 => "registered_v3_module",
        super::family_variant::Variant::VvsV3 => "vvs_v3_module",
        _ => return Err(ctx.err(ErrorCode::NotApplicable,"Для этого протокола нет модуля с привязкой к фабрике")),
    };
    if kind!=expected || !f.tradable {
        return Err(ctx.err(ErrorCode::NotApplicable,"Выбранный вид модуля не соответствует семейству пула"));
    }
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    verify_code(ctx, f.id, addr, at).await?;
    let call = super::abi_check::IPoolImmutables::factoryCall {};
    let [raw] = super::nami_batch::read(ctx, [super::nami_batch::view(at, addr, call.clone())]).await?;
    if generation!=ctx.settings.generation(){
        return Err(ctx.err(ErrorCode::StaleData,"Настройки модуля изменились при сверке фабрики"));
    }
    if super::nami_batch::decode(&raw, call)? != factory {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            "Модуль привязан к другой фабрике",
        ));
    }
    Ok(())
}

pub async fn verify(ctx: &Ctx, f: &Family, addr: Address, at: &str) -> GatewayResult<()> {
    if f.variant.umbrae() && f.tradable {let tag=super::nami::block(ctx,at).await?;return super::umbrae::module(ctx,f,addr,&tag).await;}
    if f.variant == super::family_variant::Variant::ElasticV5 && f.tradable {
        let tag=super::nami::block(ctx,at).await?;
        return verify_code(ctx,f.id,addr,&tag).await;
    }
    if f.variant == super::family_variant::Variant::Virtuals {
        let at=super::nami::block(ctx,at).await?;
        super::virtuals_roles::at(ctx,f,&at).await?;
        return verify_code(ctx,f.id,addr,&at).await;
    }
    if f.variant.wallet_module() {
        let at = super::elfomo::block(ctx,at).await?;
        return verify_factory(ctx,f,addr,&at).await;
    }
    if f.variant == super::family_variant::Variant::BaseNativeV3 {
        if !super::base_native_v3::valid(ctx, f) {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Изменена фабрика или квотер прямого V3-модуля Base",
            ));
        }
        return verify_code(ctx, f.id, addr, &json!(at)).await;
    }
    if f.proto == super::families::Proto::BaiBai {
        let tag = elfomo::block(ctx, at).await?;
        let profile = super::pmm_profile::Profile::read(ctx, f.id, f.proto)?;
        super::baibai::verify_at(ctx, &profile, &tag).await?;
        return verify_code(ctx, f.id, addr, &tag).await;
    }
    if f.proto == super::families::Proto::Lunar {
        let tag = elfomo::block(ctx, at).await?;
        let profile = super::pmm_profile::Profile::read(ctx, f.id, f.proto)?;
        super::lunar::verify_at(ctx, &profile, &tag).await?;
        if !super::lunar::open(
            ctx,
            &tag,
            super::super::wallet::signer::address(ctx)?,
            profile.pool,
        )
        .await?
        {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "LunarBase на паузе или срок цены оператора истёк",
            ));
        }
        return verify_code(ctx, f.id, addr, &tag).await;
    }
    if f.proto == super::families::Proto::Elfomo {
        let tag = elfomo::block(ctx, at).await?;
        let profile = super::pmm_profile::Profile::read(ctx, f.id, f.proto)?;
        elfomo::verify_at(ctx, &profile, &tag).await?;
        return verify_code(ctx, f.id, addr, &tag).await;
    }
    verify_code(ctx, f.id, addr, &json!(at)).await
}
