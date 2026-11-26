//! Файл: venues/bsc_dex/pools/elfomo.rs.
//! Что делает: проверяет ElfomoFi Base и читает котировки partnerId=0 от кошелька на одном каноническом блоке.
//! Кто вызывает: factory_ask, verify_stable_u, load, quoter. От чего зависит: ABI и снимок elfomofi_base.json.
//! Настройки: dex.pmm_pool, dex.oracle, dex.pool_implementation, dex.vault, dex.pmm_ladder_usd, dex.pmm_ladder_counter.
use super::super::{ctx::Ctx, node::rpc};
use super::{
    discover::Found, families::Proto, pmm_profile::Profile, verify::Candidate, verify_woo,
};
use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes};
use serde::Deserialize;
use serde_json::{Value, json};

mod abi;
mod ladder;
mod market;
pub use abi::IElfomo;
pub use market::{load, quote, within_capacity};

pub fn listed(raw: &[u8], coin: Address, counter: Address) -> Result<bool, String> {
    let pairs = IElfomo::getSupportedPairsCall::abi_decode_returns_validate(raw)
        .map_err(|e| e.to_string())?;
    Ok(pairs.iter().any(|p| {
        (p.tokenA, p.tokenB) == (coin, counter) || (p.tokenA, p.tokenB) == (counter, coin)
    }))
}

pub(super) async fn block(ctx: &Ctx, tag: &str) -> GatewayResult<Value> {
    #[derive(Deserialize)]
    struct Header {
        hash: B256,
    }
    let h: Header = rpc::call(ctx, "eth_getBlockByNumber", json!([tag, false])).await?;
    Ok(json!({"blockHash":h.hash,"requireCanonical":true}))
}

/// Проверяет фиксированные ссылки из конструктора, код роутера и текущую реализацию pricing.
pub(super) async fn verify_at(ctx: &Ctx, profile: &Profile, tag: &Value) -> GatewayResult<()> {
    profile.verify_code(ctx, tag).await
}

pub async fn verify(ctx: &Ctx, c: &Candidate, router: Address) -> GatewayResult<Found> {
    let profile = Profile::read(ctx, c.family.id, Proto::Elfomo)?;
    if router != profile.pool || !profile.pair(c.coin, c.quote) {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Этой пары нет в рабочем профиле ElfomoFi",
        ));
    }
    let tag = block(ctx, "latest").await?;
    verify_at(ctx, &profile, &tag).await?;
    let query = Bytes::from(IElfomo::getSupportedPairsCall {}.abi_encode());
    let raw: Bytes = rpc::call(ctx, "eth_call", json!([{"to":router,"data":query},tag])).await?;
    if !listed(&raw, c.coin, c.quote).map_err(|e| ctx.err(ErrorCode::BadResponse, e))? {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Для пары ElfomoFi нет подтверждённого полного обмена",
        ));
    }
    Ok(verify_woo::found(
        c,
        PoolRef::Member {
            contract: router,
            coin: c.coin,
        },
    ))
}
