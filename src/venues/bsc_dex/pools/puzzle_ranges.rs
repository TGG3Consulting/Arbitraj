//! Puzzle Ranges uses a Balancer V3 vault with an after-swap hook and its own virtual balances.
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};

use super::{
    balancer_v3_factories::{self, Release},
    balancer_v3_hooks::{self, Lists},
    families::Family,
    family_variant::Variant,
};
use crate::{core::types::pool_vault::VaultV3Rule, venues::bsc_dex::ctx::Ctx};

alloy::sol! {
    interface IRanges {
        struct Dynamic {
            uint256[] balancesRaw;
            uint256[] balancesLiveScaled18;
            uint256[] virtualBalances;
            uint256[] leverageRatios;
            uint256[] tokenRates;
            uint256 staticSwapFeePercentage;
            uint256 totalSupply;
            bool isPoolRegistered;
            bool isPoolInitialized;
            bool isPoolPaused;
            bool isPoolInRecoveryMode;
            bool isVaultPaused;
            bool isHookStopped;
        }
        function getPoolCount() view returns (uint256);
        function getPoolsInRange(uint256 start, uint256 count) view returns (address[]);
        function getHook() view returns (address);
        function getRangePoolDynamicData() view returns (Dynamic);
    }
}

pub(super) fn bindings(
    ctx: &Ctx,
    family: &Family,
) -> Result<(Vec<(Address, Release)>, Lists), String> {
    if family.variant == Variant::PuzzleRanges {
        let address = |k| ctx.dex_addr(k, family.id).map_err(|e| e.message);
        let factory = address("dex.factory")?;
        let hook = address("dex.ranges_hook")?;
        if factory.is_zero() || hook.is_zero() {
            return Err("Puzzle Ranges: пустой адрес фабрики или крючка".into());
        }
        return Ok((
            vec![(factory, Release::PuzzleRanges(hook))],
            Lists::default(),
        ));
    }
    Ok((
        balancer_v3_factories::all(ctx, family.id)?,
        balancer_v3_hooks::lists(ctx, family.id)?,
    ))
}

pub(super) fn state(raw: &[u8], live: &[U256], weights: &[U256]) -> Result<VaultV3Rule, String> {
    let s =
        IRanges::getRangePoolDynamicDataCall::abi_decode_returns(raw).map_err(|e| e.to_string())?;
    if s.balancesLiveScaled18 != live
        || s.virtualBalances.len() != live.len()
        || weights.len() != live.len()
    {
        return Err("Puzzle Ranges: состав или остатки пула расходятся с хранилищем".into());
    }
    Ok(VaultV3Rule::PuzzleRanges {
        virtual_balances: s.virtualBalances,
        sequence_fee: None,
        enabled: s.isPoolRegistered
            && s.isPoolInitialized
            && !s.isPoolPaused
            && !s.isVaultPaused
            && !s.isHookStopped
            && !s.isPoolInRecoveryMode,
    })
}

pub(super) async fn scan(
    ctx: &Ctx,
    coins: &std::collections::HashMap<Address, String>,
    quotes: &[Address],
) -> super::stable_list::Extra {
    let mut out = super::stable_list::Extra::default();
    for family in super::families::all(ctx) {
        if family.variant == Variant::PuzzleRanges
            && super::families::classify(ctx, family.id).is_ok()
            && let Err(e) = Box::pin(registry(ctx, family, coins, quotes, &mut out)).await
        {
            out.no_answer += 1;
            out.notes.push(format!("{}: {}", family.id, e.message));
        }
    }
    out
}

async fn registry(
    ctx: &Ctx,
    family: &Family,
    coins: &std::collections::HashMap<Address, String>,
    quotes: &[Address],
    out: &mut super::stable_list::Extra,
) -> crate::gateway::errors::GatewayResult<()> {
    use super::{abi_balancer_v3::IBalancerV3Vault as V, nami_batch as batch};
    use crate::{gateway::errors::ErrorCode, venues::bsc_dex::node::rpc};
    let at = super::nami::block(ctx, "latest").await?;
    let factory = ctx.dex_addr("dex.factory", family.id)?;
    let vault = ctx.dex_addr("dex.vault", family.id)?;
    let [raw] = batch::read(
        ctx,
        [batch::view(&at, factory, IRanges::getPoolCountCall {})],
    )
    .await?;
    let count = batch::decode(&raw, IRanges::getPoolCountCall {})?;
    let mut start = U256::ZERO;
    while start < count {
        let count = (count - start).min(U256::from(48));
        let get = IRanges::getPoolsInRangeCall { start, count };
        let [raw] = batch::read(ctx, [batch::view(&at, factory, get.clone())]).await?;
        let pools = batch::decode(&raw, get)?;
        if U256::from(pools.len()) != count {
            return Err(ctx.err(
                ErrorCode::BadResponse,
                "Puzzle Ranges: неполная страница фабрики",
            ));
        }
        let requests: Vec<_> = pools
            .iter()
            .map(|&pool| batch::view(&at, vault, V::getPoolTokensCall { pool }))
            .collect();
        let answers = rpc::batch(ctx, &requests).await?;
        if answers.len() != pools.len() {
            return Err(ctx.err(
                ErrorCode::BadResponse,
                "Puzzle Ranges: неполный ответ по токенам",
            ));
        }
        out.calls += requests.len() + 1;
        for (pool, item) in pools.into_iter().zip(answers) {
            let raw: alloy_primitives::Bytes = serde_json::from_str(item?.get()).map_err(|e| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("Puzzle Ranges: список токенов: {e}"),
                )
            })?;
            let tokens = batch::decode(&raw, V::getPoolTokensCall { pool })?;
            for entry in super::stable_list::candidates(family.id, pool, &tokens, coins, quotes) {
                out.found.entry(entry.base).or_default().push(entry);
            }
        }
        start += count;
    }
    Ok(())
}
