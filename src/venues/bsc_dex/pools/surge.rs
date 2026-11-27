//! Один адрес SURGE является токеном и рынком. У рынка собственные виртуальные резервы
//! и ETH для выплаты. Все наблюдения относятся к одному canonical blockHash и фактическому покупателю.
use super::{
    families::{self, Family},
    family_variant::Variant,
    nami_batch as call,
    stable_list::Extra,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::{PoolRef, surge::SurgeState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        directory::gt_parse::IndexedPool,
        node::{abi::IERC20, rpc},
        wallet::signer,
    },
};
use alloy::sol;
use alloy_primitives::{Address, B256, U256};
use serde_json::{Value, json};
use std::collections::HashMap;

sol! { interface ISurge {
    function liquidity() external view returns(uint256);
    function liqConst() external view returns(uint256);
    function maxBag() external view returns(uint256);
    function DIVISOR() external view returns(uint256);
    function buyMul() external view returns(uint256);
    function sellMul() external view returns(uint256);
    function isFeeExempt(address who) external view returns(bool);
    function isTxLimitExempt(address who) external view returns(bool);
    function getBNBPrice() external view returns(uint256);
    function market() external view returns(address);
    function wrapped() external view returns(address);
    function payer() external view returns(address);
    function executor() external view returns(address);
}}
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Surge: код, валюты, пользователь или параметры кривой изменились",
    )
}
pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    token: Address,
    edge: (Address, Address),
    at: &Value,
) -> GatewayResult<SurgeState> {
    let revision = ctx.settings.generation();
    let wrapped = ctx.addr("dex.native_wrapped")?;
    if at["requireCanonical"] != true
        || at["blockHash"]
            .as_str()
            .and_then(|s| s.parse::<B256>().ok())
            .is_none()
        || token != ctx.dex_addr("dex.factory", f.id)?
        || token == wrapped
        || token.is_zero()
        || !(edge == (token, wrapped) || edge == (wrapped, token))
    {
        return Err(bad(ctx));
    }
    let hash = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| bad(ctx))?;
    let payer = signer::address(ctx)?;
    let trader = if f.executor_kind.is_some() {
        ctx.dex_addr("dex.router", f.id)?
    } else {
        payer
    };
    let role = async {
        super::solidly_classic_factory::plain(ctx, token, hash, at).await?;
        if trader != payer {
            let [m, w, p, x] = call::read(
                ctx,
                [
                    call::view(at, trader, ISurge::marketCall {}),
                    call::view(at, trader, ISurge::wrappedCall {}),
                    call::view(at, trader, ISurge::payerCall {}),
                    call::view(at, trader, ISurge::executorCall {}),
                ],
            )
            .await?;
            let exec = call::decode(&x, ISurge::executorCall {})?;
            if call::decode(&m, ISurge::marketCall {})? != token
                || call::decode(&w, ISurge::wrappedCall {})? != wrapped
                || call::decode(&p, ISurge::payerCall {})? != payer
                || ctx.executor()?.is_some_and(|expected| exec != expected)
            {
                return Err(bad(ctx));
            }
            super::direct_module::verify_code(ctx, f.id, trader, at).await?;
        } else if f.executor_kind.is_some() {
            return Err(bad(ctx));
        }
        Ok::<_, crate::gateway::errors::GatewayError>(())
    };
    let base = call::read(
        ctx,
        [
            call::view(at, token, ISurge::liquidityCall {}),
            call::view(at, token, ISurge::liqConstCall {}),
            call::view(at, token, IERC20::balanceOfCall { owner: token }),
            call::view(at, token, ISurge::maxBagCall {}),
            call::view(at, token, ISurge::buyMulCall {}),
            call::view(at, token, ISurge::sellMulCall {}),
            call::view(at, token, ISurge::DIVISORCall {}),
            call::view(at, token, ISurge::isFeeExemptCall { who: trader }),
            call::view(at, token, ISurge::isTxLimitExemptCall { who: trader }),
            call::view(at, token, IERC20::balanceOfCall { owner: trader }),
            call::view(at, token, ISurge::getBNBPriceCall {}),
        ],
    );
    let (
        (),
        [
            liq,
            k,
            stock,
            bag,
            buy,
            sell,
            div,
            exempt,
            unlimited,
            own,
            price,
        ],
        cash,
    ) = tokio::try_join!(
        role,
        base,
        rpc::call::<U256>(ctx, "eth_getBalance", json!([token, at]))
    )?;
    let free = call::decode(&exempt, ISurge::isFeeExemptCall { who: trader })?;
    let unlimited = call::decode(&unlimited, ISurge::isTxLimitExemptCall { who: trader })?;
    let capacity = if unlimited {
        U256::MAX
    } else {
        call::decode(&bag, ISurge::maxBagCall {})?
            .saturating_sub(call::decode(&own, IERC20::balanceOfCall { owner: trader })?)
    };
    let rate = |v: U256| -> GatewayResult<u8> {
        if !(U256::from(90)..=U256::from(100)).contains(&v) {
            return Err(bad(ctx));
        }
        Ok(if free { 100 } else { v.to() })
    };
    // Некорректная внешняя цена статистики откатывает сам swap. Нулевой корректный uint256 допустим.
    call::decode(&price, ISurge::getBNBPriceCall {})?;
    if revision != ctx.settings.generation()
        || call::decode(&div, ISurge::DIVISORCall {})? != U256::from(100)
    {
        return Err(bad(ctx));
    }
    Ok(SurgeState {
        token,
        wrapped,
        trader,
        liquidity: call::decode(&liq, ISurge::liquidityCall {})?,
        invariant: call::decode(&k, ISurge::liqConstCall {})?,
        stock: call::decode(&stock, IERC20::balanceOfCall { owner: token })?,
        cash,
        capacity,
        buy_mul: rate(call::decode(&buy, ISurge::buyMulCall {})?)?,
        sell_mul: rate(call::decode(&sell, ISurge::sellMulCall {})?)?,
    })
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Ok(token) = c.pool.parse::<Address>() else {
        return Verdict::Rejected("Surge: нужен адрес токена".into());
    };
    let checked = async {
        let at = super::nami::block(ctx, "latest").await?;
        market(ctx, c.family, token, (c.coin, c.quote), &at).await
    }
    .await;
    match checked {
        Ok(_) => Verdict::Taken(Box::new(super::verify_woo::found(
            c,
            PoolRef::Contract(token),
        ))),
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
pub(super) fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut out = Extra::default();
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.variant == Variant::Surge)
    {
        let (Ok(token), Ok(wrapped)) = (
            ctx.dex_addr("dex.factory", f.id),
            ctx.addr("dex.native_wrapped"),
        ) else {
            continue;
        };
        for (a, b) in [(token, wrapped), (wrapped, token)] {
            if let Some(symbol) = coins.get(&a)
                && quotes.contains(&b)
            {
                out.found.entry(a).or_default().push(IndexedPool {
                    dex_id: f.id.into(),
                    pool: token.to_string(),
                    base: a,
                    quote: b,
                    base_symbol: symbol.clone(),
                    quote_symbol: coins.get(&b).cloned().unwrap_or_else(|| format!("{b:#x}")),
                    reserve_usd: None,
                });
            }
        }
    }
    out
}
