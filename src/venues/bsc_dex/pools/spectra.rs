//! Допуск физического Curve-рынка Spectra. Монета 0 — IBT из PT, монета 1 — PT из реестра.
//! Погашение PT и вывод underlying через ERC4626 образуют другие операции.
use super::super::ctx::Ctx;
use super::{
    families::{Family, Proto},
    nami_batch as batch,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy_primitives::{Address, U256};
use serde_json::json;

sol! {
    interface ISpectra {
        function coins(uint256 index) external view returns(address);
        function getIBT() external view returns(address);
        function paused() external view returns(bool);
        function isRegisteredPT(address pt) external view returns(bool);
        function get_implementation_address(address pool) external view returns(address);
        function get_coins(address pool) external view returns(address[2]);
    }
}

pub(super) async fn check(ctx: &Ctx, f: &Family, pool: Address, tag: &str) -> GatewayResult<()> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Spectra: реестр, реализация или физическая пара IBT/PT не подтверждены",
        )
    };
    let generation = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let registry = ctx.dex_addr("dex.spectra_registry", f.id)?;
    let stable = f.proto == Proto::CurveNg;
    let key = if stable {
        "dex.curve_ng_templates"
    } else {
        "dex.curve_crypto_v1_templates"
    };
    let allowed = ctx
        .dex_list(key, f.id)?
        .iter()
        .map(|v| v.parse::<Address>().map_err(|_| bad()))
        .collect::<Result<Vec<_>, _>>()?;
    if allowed.is_empty()
        || allowed.contains(&Address::ZERO)
        || registry.is_zero()
        || factory.is_zero()
    {
        return Err(bad());
    }
    let at = super::nami::block(ctx, tag).await?;
    let first = ISpectra::coinsCall { index: U256::ZERO };
    let second = ISpectra::coinsCall { index: U256::ONE };
    let implementation = ISpectra::get_implementation_addressCall { pool };
    let identity = if stable {
        batch::view(&at, factory, implementation.clone())
    } else {
        ("eth_getCode", json!([pool, at]))
    };
    let [a, b, physical] = batch::read(
        ctx,
        [
            batch::view(&at, pool, first.clone()),
            batch::view(&at, pool, second.clone()),
            identity,
        ],
    )
    .await?;
    let ibt = batch::decode(&a, first)?;
    let pt = batch::decode(&b, second)?;
    let template = if stable {
        batch::decode(&physical, implementation)?
    } else {
        super::verify_crypto_v1::proxy_target(&physical).ok_or_else(bad)?
    };
    if !allowed.contains(&template) || pt == ibt || pt.is_zero() || ibt.is_zero() {
        return Err(bad());
    }
    let registered = ISpectra::isRegisteredPTCall { pt };
    let coins = if stable {
        batch::view(
            &at,
            factory,
            super::abi_curve_ng::ICurveNgFactory::get_coinsCall { pool },
        )
    } else {
        batch::view(&at, factory, ISpectra::get_coinsCall { pool })
    };
    let [tokens, record, underlying, paused] = batch::read(
        ctx,
        [
            coins,
            batch::view(&at, registry, registered.clone()),
            batch::view(&at, pt, ISpectra::getIBTCall {}),
            batch::view(&at, pt, ISpectra::pausedCall {}),
        ],
    )
    .await?;
    let matches = if stable {
        batch::decode(
            &tokens,
            super::abi_curve_ng::ICurveNgFactory::get_coinsCall { pool },
        )?
        .as_slice()
            == [ibt, pt]
    } else {
        batch::decode(&tokens, ISpectra::get_coinsCall { pool })? == [ibt, pt]
    };
    if !matches
        || !batch::decode(&record, registered)?
        || batch::decode(&underlying, ISpectra::getIBTCall {})? != ibt
        || batch::decode(&paused, ISpectra::pausedCall {})?
    {
        return Err(bad());
    }
    if ctx.settings.generation() != generation {
        return Err(ctx.err(ErrorCode::StaleData, "Изменились настройки Spectra"));
    }
    Ok(())
}
