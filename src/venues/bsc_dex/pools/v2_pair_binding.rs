//! Проверяет код фабрики и пары для прямого Router02 и V5 с изменяемой ставкой.
//! Настройки хешей принадлежат выбранному DEX. Все ответы одной проверки закреплены за одним блоком.
use super::{
    abi_v2v3::IV2Pair,
    nami_batch as batch,
    registry::Entry,
    v2_live_fee::{Market, Rate},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{ctx::Ctx, node::abi::IERC20};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::{Value, json};

pub(in crate::venues::bsc_dex) fn expected(ctx: &Ctx, id: &str, key: &str) -> GatewayResult<B256> {
    ctx.dex_text(key, id)?
        .parse::<B256>()
        .ok()
        .filter(|h| !h.is_zero() && *h != keccak256([]))
        .ok_or_else(|| ctx.err(ErrorCode::NotConfigured, format!("{id}: неверный {key}")))
}

fn matches(actual: &[u8], expected: B256) -> bool {
    !actual.is_empty() && keccak256(actual) == expected
}

pub(in crate::venues::bsc_dex) async fn code(
    ctx: &Ctx,
    id: &str,
    pool: Address,
    at: &Value,
) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let factory = ctx.dex_addr("dex.factory", id)?;
    let factory_hash = expected(ctx, id, "dex.factory_runtime_hash")?;
    let pair_hash = expected(ctx, id, "dex.pair_runtime_hash")?;
    let [factory_code, pair_code] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([factory, at])),
            ("eth_getCode", json!([pool, at])),
        ],
    )
    .await?;
    if generation != ctx.settings.generation()
        || !matches(&factory_code, factory_hash)
        || !matches(&pair_code, pair_hash)
    {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Код фабрики или пары расходится с профилем V2",
        ));
    }
    Ok(())
}

fn funded(
    reserves: (U256, U256),
    balances: (U256, U256),
    quoted: Option<u32>,
    actual: u32,
) -> bool {
    reserves == balances && quoted == Some(actual)
}

pub(crate) async fn fresh(ctx: &Ctx, e: &Entry, at: &Value) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let pool = e.pool.reference.address();
    let (a, b) = (e.pool.token0, e.pool.token1);
    let rule = Rate::configured(
        ctx,
        e.family,
        Market {
            pool,
            tokens: (a, b),
        },
    )?;
    let (to, getter) = rule.call();
    let balance = IERC20::balanceOfCall { owner: pool };
    let reserve = IV2Pair::getReservesCall {};
    let calls = [
        ("eth_call", json!([{"to":to,"data":getter},at])),
        batch::view(at, pool, reserve.clone()),
        batch::view(at, a, balance.clone()),
        batch::view(at, b, balance.clone()),
    ];
    let ((), [value, rs, b0, b1]) =
        tokio::try_join!(code(ctx, e.family.id, pool, at), batch::read(ctx, calls))?;
    let rs = batch::decode(&rs, reserve)?;
    let actual = rule.decode(&value)?;
    let balances = (
        batch::decode(&b0, balance.clone())?,
        batch::decode(&b1, balance)?,
    );
    if generation != ctx.settings.generation()
        || !funded(
            (rs.reserve0.to(), rs.reserve1.to()),
            balances,
            e.pool.fee_pips,
            actual,
        )
    {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "Изменилась ставка V2 или остатки пары расходятся с резервами",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    #[test]
    fn rejects_other_runtime_fee_or_unattributed_balance() {
        let raw = Bytes::from_static(b"pair");
        assert!(matches(&raw, keccak256(&raw)));
        assert!(!matches(&raw, keccak256(b"factory")));
        assert!(!matches(&[], keccak256([])));
        let rs = (U256::from(8), U256::from(11));
        assert!(funded(rs, rs, Some(2500), 2500));
        assert!(funded(rs, rs, Some(5000), 5000));
        assert!(!funded(rs, rs, Some(2500), 5000));
        assert!(!funded(rs, (rs.0 + U256::from(1), rs.1), Some(2500), 2500));
        assert!(!funded(rs, (rs.0, rs.1 - U256::from(1)), Some(2500), 2500));
    }
}
