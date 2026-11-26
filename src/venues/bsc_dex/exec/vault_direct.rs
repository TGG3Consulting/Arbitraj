//! Native Balancer V2 Vault execution for markets with an explicitly verified wallet profile.
//! ERC-20 funds come only from the sender. The same wire is used in the wallet probe and the trade.
use super::super::{
    ctx::Ctx,
    node::rpc,
    pools::{
        abi_balancer::{IBalancerFactory as F, IBalancerVault as V},
        family_variant::Variant,
        registry::Entry,
    },
    quote::white_state::{self, Head},
};
use super::{
    chain::Step,
    plan::{DirectCall, Order, Plan},
};
use crate::{
    core::types::PoolState,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::json;

pub(in crate::venues::bsc_dex) fn enabled(e: &Entry) -> bool {
    e.family.variant == Variant::BalancerVaultWallet
}

pub(in crate::venues::bsc_dex) fn market(ctx: &Ctx, e: &Entry) -> GatewayResult<(Address, B256)> {
    let PoolState::VaultPool(s) = &e.pool.state else {
        return Err(ctx.err(ErrorCode::NoRoute, "Отсутствует состояние пула Vault"));
    };
    let vault = ctx.dex_addr("dex.vault", e.family.id)?;
    if !enabled(e)
        || s.v3.is_some()
        || vault.is_zero()
        || s.pool_id.as_slice()[..20] != e.pool.reference.address().as_slice()[..]
    {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "ID рынка не соответствует пулу Vault V2",
        ));
    }
    Ok((vault, s.pool_id))
}

// Сам по себе неизменный hash прокси не доказывает выплату: реализацию или ставку налога можно сменить.
fn audited_transfer(chain: Option<u64>, token: Address, runtime: B256) -> bool {
    use std::sync::LazyLock;
    #[derive(serde::Deserialize)]
    struct Token {
        address: Address,
        hash: B256,
        exact: bool,
    }
    #[derive(serde::Deserialize)]
    struct Audit {
        chain_id: u64,
        tokens: Vec<Token>,
    }
    static AUDIT: LazyLock<Option<Audit>> = LazyLock::new(|| {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/config/vault_exact_transfers.json"
        )))
        .ok()
    });
    AUDIT.as_ref().is_some_and(|a| {
        Some(a.chain_id) == chain
            && a.tokens
                .iter()
                .any(|t| t.exact && t.address == token && t.hash == runtime)
    })
}

/// The Vault enforces its accounting amount. Only immutable, exact-transfer outputs have the same wallet minimum.
pub(in crate::venues::bsc_dex) async fn exact_output(
    ctx: &Ctx,
    e: &Entry,
    output: Address,
    h: Option<Head>,
) -> GatewayResult<()> {
    let expected = ctx
        .dex_list("dex.vault_exact_outputs", e.family.id)?
        .into_iter()
        .find_map(|line| {
            let (token, hash) = line.split_once('=')?;
            (token.trim().parse::<Address>().ok()? == output)
                .then(|| hash.trim().parse::<B256>().ok())
                .flatten()
        })
        .filter(|x| audited_transfer(ctx.chain_id(), output, *x))
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                "Для этой выплаты нужен минимум по балансу кошелька через исполнителя",
            )
        })?;
    if let Some(head) = h {
        let code: Bytes = rpc::call(ctx, "eth_getCode", json!([output, head.selector()])).await?;
        if code.is_empty() || keccak256(code) != expected {
            return Err(ctx.err(ErrorCode::QuoteMismatch, "Код выходного ERC-20 изменился"));
        }
    }
    Ok(())
}

pub(in crate::venues::bsc_dex) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
) -> GatewayResult<()> {
    let (vault, id) = market(ctx, e)?;
    let pool = e.pool.reference.address();
    let factory = ctx.dex_addr("dex.factory", e.family.id)?;
    let hash = ctx
        .dex_text("dex.router_runtime_hash", e.family.id)?
        .parse::<B256>()
        .map_err(|_| ctx.err(ErrorCode::NotConfigured, "Не задан хеш кода Vault"))?;
    let at = h.selector();
    let registry = V::getPoolCall { poolId: id }.abi_encode();
    let origin = F::isPoolFromFactoryCall { pool }.abi_encode();
    let (code, registration, owner) = tokio::try_join!(
        rpc::call::<Bytes>(ctx, "eth_getCode", json!([vault, at])),
        rpc::call::<Bytes>(
            ctx,
            "eth_call",
            json!([{"to":vault,"data":Bytes::from(registry)},at])
        ),
        rpc::call::<Bytes>(
            ctx,
            "eth_call",
            json!([{"to":factory,"data":Bytes::from(origin)},at])
        )
    )?;
    if code.is_empty()
        || keccak256(code) != hash
        || V::getPoolCall::abi_decode_returns(&registration)
            .ok()
            .is_none_or(|r| r._0 != pool)
        || F::isPoolFromFactoryCall::abi_decode_returns(&owner).ok() != Some(true)
    {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "Код Vault, регистрация или фабрика пула изменились",
        ));
    }
    Ok(())
}

pub(in crate::venues::bsc_dex) fn data(
    id: B256,
    input: Address,
    output: Address,
    amount: U256,
    minimum: U256,
    payer: Address,
    deadline: U256,
) -> Bytes {
    V::swapCall {
        singleSwap: V::SingleSwap {
            poolId: id,
            kind: 0,
            assetIn: input,
            assetOut: output,
            amount,
            userData: Bytes::new(),
        },
        funds: V::FundManagement {
            sender: payer,
            fromInternalBalance: false,
            recipient: payer,
            toInternalBalance: false,
        },
        limit: minimum,
        deadline,
    }
    .abi_encode()
    .into()
}

pub(super) async fn plan(
    ctx: &Ctx,
    steps: &[Step],
    wallet: Address,
    now: u64,
    all: bool,
) -> GatewayResult<Plan> {
    let [s] = steps else {
        return Err(ctx.err(ErrorCode::NoRoute, "Прямой Vault.swap требует один пул"));
    };
    let generation = ctx.settings.generation();
    let h = white_state::head(ctx, "latest").await?;
    tokio::try_join!(
        checked(ctx, &s.entry, h),
        exact_output(ctx, &s.entry, s.order.token_out, Some(h))
    )?;
    let (vault, id) = market(ctx, &s.entry)?;
    let pair = (s.entry.pool.token0, s.entry.pool.token1);
    if (s.order.token_in, s.order.token_out) != pair
        && (s.order.token_out, s.order.token_in) != pair
    {
        return Err(ctx.err(ErrorCode::NoRoute, "Монеты ордера не совпали с парой Vault"));
    }
    if !all {
        Box::pin(super::super::quote::vault_pricing::confirm(
            ctx, h, &s.entry, wallet, &s.order,
        ))
        .await?;
    }
    let mut p = super::white_direct::build(ctx, s, wallet, vault, now, all, generation).await?;
    p.set_direct_market(DirectCall::Vault {
        id,
        input: s.order.token_in,
        amount: s.order.amount_in,
    });
    Ok(p)
}

pub(in crate::venues::bsc_dex) async fn probe(
    ctx: &Ctx,
    e: &Entry,
    o: Order,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<super::super::wallet::approve::Approval>, Address, Bytes)> {
    // This builds calls exclusively for the isolated buy/transfer/sell probe. An output
    // with transfer tax must still be measurable before selecting its protected V5 path.
    let generation = ctx.settings.generation();
    let h = white_state::head(ctx, "latest").await?;
    checked(ctx, e, h).await?;
    let (vault, id) = market(ctx, e)?;
    let steps = super::plan::single(e, o);
    let step = &steps[0];
    let mut p = super::white_direct::build(ctx, step, wallet, vault, now, true, generation).await?;
    p.set_direct_market(DirectCall::Vault {
        id,
        input: step.order.token_in,
        amount: step.order.amount_in,
    });
    let data = p.data(U256::ZERO);
    Ok((p.approvals, p.router, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_or_transfer_tax_cannot_be_approved_by_copying_its_current_runtime_hash() {
        let proof: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/config/vault_exact_transfers.json"
        )))
        .unwrap();
        let mut audited = 0;
        for row in proof["tokens"].as_array().unwrap() {
            let token = row["address"].as_str().unwrap().parse().unwrap();
            let runtime = row["hash"].as_str().unwrap().parse().unwrap();
            let eligible = row["exact"].as_bool().unwrap();
            audited += usize::from(eligible);
            assert_eq!(audited_transfer(Some(8453), token, runtime), eligible);
            assert!(!audited_transfer(Some(1), token, runtime));
            assert!(!audited_transfer(Some(8453), token, B256::ZERO));
        }
        assert_eq!(audited, 6);
    }
}
