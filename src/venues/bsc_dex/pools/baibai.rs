//! Проверяет ссылки и код трёх обновляемых контрактов BaiBai в каноническом состоянии Base.
//! Используют поиск, загрузка и квотер. Настройки: dex.pmm_pool, dex.oracle, dex.vault, dex.pool_implementation.
use super::super::ctx::Ctx;
use super::{
    discover::Found, elfomo, families::Proto, pmm_profile::Profile, pmm_view::view,
    verify::Candidate, verify_woo,
};
use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy_primitives::Address;
use serde_json::Value;

mod market;
pub use market::{load, quote};

sol! {
    interface IBai {
        struct Side { int16 spreadBps; uint16 depthBps; uint256 filled; uint8 knotCount; }
        struct Pair { uint64 seq; uint64 fillSeq; uint64 lastUpdateAt; uint80 mid; uint256 qUnit; Side ask; Side bid; }
        function quoteToken() external view returns (address);
        function curveBook() external view returns (address);
        function custodian() external view returns (address);
        function entrypoint() external view returns (address);
        function quoteFor(address base,address tokenIn,uint256 amountIn,address taker) external view returns (uint256 amountOut,uint256 fee);
        function pair(address base) external view returns (Pair memory);
        function balanceOf(address account) external view returns (uint256);
        event Fill(address indexed base,uint64 indexed fillSeq,address indexed taker,address tokenIn,uint256 amountIn,uint256 amountOut,uint256 fee);
    }
}

pub(super) async fn verify_at(ctx: &Ctx, profile: &Profile, tag: &Value) -> GatewayResult<()> {
    let wrong = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Изменилось размещение, связи или код BaiBai",
        )
    };
    let zero = Address::ZERO;
    let ((), eb, ev, eq, be, bq, ve, vb) = Box::pin(async {
        tokio::try_join!(
            profile.verify_code(ctx, tag),
            view(ctx, tag, zero, profile.pool, IBai::curveBookCall {}),
            view(ctx, tag, zero, profile.pool, IBai::custodianCall {}),
            view(ctx, tag, zero, profile.pool, IBai::quoteTokenCall {}),
            view(ctx, tag, zero, profile.oracle, IBai::entrypointCall {}),
            view(ctx, tag, zero, profile.oracle, IBai::quoteTokenCall {}),
            view(ctx, tag, zero, profile.vault, IBai::entrypointCall {}),
            view(ctx, tag, zero, profile.vault, IBai::curveBookCall {}),
        )
    })
    .await?;
    if (eb, ev, eq, be, bq, ve, vb)
        != (
            profile.oracle,
            profile.vault,
            profile.counter,
            profile.pool,
            profile.counter,
            profile.pool,
            profile.oracle,
        )
    {
        return Err(wrong());
    }
    Ok(())
}

pub async fn verify(ctx: &Ctx, c: &Candidate, address: Address) -> GatewayResult<Found> {
    let profile = Profile::read(ctx, c.family.id, Proto::BaiBai)?;
    if address != profile.pool || !profile.pair(c.coin, c.quote) {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Этой пары нет в рабочем профиле BaiBai",
        ));
    }
    let tag = elfomo::block(ctx, "latest").await?;
    verify_at(ctx, &profile, &tag).await?;
    let book = view(
        ctx,
        &tag,
        Address::ZERO,
        profile.oracle,
        IBai::pairCall { base: c.coin },
    )
    .await?;
    if book.qUnit.is_zero() {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Эта монета отсутствует в книге BaiBai",
        ));
    }
    Ok(verify_woo::found(
        c,
        PoolRef::Member {
            contract: profile.pool,
            coin: c.coin,
        },
    ))
}
