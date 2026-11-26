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
            if entry.family.variant == super::family_variant::Variant::BaseNativeV3 =>
        {
            Ok(*address)
        }
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

pub async fn verify(ctx: &Ctx, f: &Family, addr: Address, at: &str) -> GatewayResult<()> {
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
