//! Mint Club V2 ERC20 markets. The market address is the issued clone; the Bond holds reserve assets for many markets.
use super::super::ctx::Ctx;
use super::{families::Family, nami_batch as batch};
use crate::{
    core::types::MintClubState,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};

sol! {
    struct Step { uint128 rangeTo; uint128 price; }
    interface IBond {
        function tokenBond(address token) external view returns(address creator,uint16 mintRoyalty,uint16 burnRoyalty,uint40 createdAt,address reserveToken,uint256 reserveBalance);
        function getSteps(address token) external view returns(Step[] memory);
        function getReserveForToken(address token,uint256 tokensToMint) external view returns(uint256 reserveAmount,uint256 royalty);
        function getRefundForTokens(address token,uint256 tokensToBurn) external view returns(uint256 refundAmount,uint256 royalty);
        function totalSupply() external view returns(uint256);
        function decimals() external view returns(uint8);
        function bond() external view returns(address);
        function balanceOf(address account) external view returns(uint256);
        event Mint(address indexed token,address indexed user,address receiver,uint256 amountMinted,address indexed reserveToken,uint256 reserveAmount);
        event Burn(address indexed token,address indexed user,address receiver,uint256 amountBurned,address indexed reserveToken,uint256 refundAmount);
        event Transfer(address indexed from,address indexed to,uint256 value);
        event TokenCreated(address indexed token,string name,string symbol,address indexed reserveToken);
    }
}

/// tokenBond is written only when the authenticated Bond creates the token.
/// The dynamic ERC1155 registrations also appear here, so the loader additionally verifies the ERC20 clone runtime.
pub(super) fn listed(
    token: Address,
    reserve: Address,
    data: &[u8],
) -> Result<Vec<Address>, String> {
    let m = IBond::tokenBondCall::abi_decode_returns_validate(data).map_err(|e| e.to_string())?;
    Ok(
        if !token.is_zero() && !reserve.is_zero() && m.reserveToken == reserve {
            vec![token]
        } else {
            Vec::new()
        },
    )
}

fn clone_of(code: &[u8], implementation: Address) -> bool {
    code.len() == 45
        && code[..10] == alloy_primitives::hex!("363d3d373d3d3d363d73")
        && code[10..30] == implementation[..]
        && code[30..] == alloy_primitives::hex!("5af43d82803e903d91602b57fd5bf3")
}

/// Read the pair, both runtime identities, supply, economic curve, per-market accounting and physical reserve at a single hash.
pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    token: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<MintClubState> {
    let no = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Mint Club V2: реестр, клон ERC20 или параметры рынка не подтверждены",
        )
    };
    let generation = ctx.settings.generation();
    let block = at["blockHash"]
        .as_str()
        .and_then(|s| s.parse::<B256>().ok())
        .ok_or_else(no)?;
    if at["requireCanonical"] != true || token.is_zero() || pair.0 == pair.1 {
        return Err(no());
    }
    let reserve = if pair.0 == token {
        pair.1
    } else if pair.1 == token {
        pair.0
    } else {
        return Err(no());
    };
    let bond = ctx.dex_addr("dex.factory", f.id)?;
    let factory_hash = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| no())?;
    let templates = ctx.dex_list("dex.proxy_implementations", f.id)?;
    let hashes = ctx.dex_list("dex.proxy_runtime_hashes", f.id)?;
    let ([template], [expected]) = (templates.as_slice(), hashes.as_slice()) else {
        return Err(no());
    };
    let template = template.parse::<Address>().map_err(|_| no())?;
    let expected = expected.parse::<B256>().map_err(|_| no())?;
    if [bond, template, reserve].contains(&Address::ZERO)
        || bond == token
        || template == token
        || template == bond
    {
        return Err(no());
    }
    let code = |a: Address| ("eth_getCode", json!([a, at]));
    let key = super::deployment_at_head::Key {
        block,
        contracts: [(bond, factory_hash), (template, expected)],
    };
    let seen = ctx.evm_chain.mint_club().at(key);
    let verify = seen.get_or_try_init(|| async {
        let [factory, implementation] = batch::read(ctx, [code(bond), code(template)]).await?;
        if factory.is_empty()
            || implementation.is_empty()
            || keccak256(factory) != factory_hash
            || keccak256(implementation) != expected
        {
            return Err(no());
        }
        Ok(())
    });
    let state = batch::read(
        ctx,
        [
            code(token),
            batch::view(at, bond, IBond::tokenBondCall { token }),
            batch::view(at, bond, IBond::getStepsCall { token }),
            batch::view(at, token, IBond::totalSupplyCall {}),
            batch::view(at, token, IBond::bondCall {}),
            batch::view(at, token, IBond::decimalsCall {}),
            batch::view(at, reserve, IBond::balanceOfCall { account: bond }),
        ],
    );
    let (_, [clone, metadata, terms, total, issuer, decimals, balance]) =
        tokio::try_join!(verify, state)?;
    if !clone_of(&clone, template) {
        return Err(no());
    }
    let m = batch::decode(&metadata, IBond::tokenBondCall { token })?;
    let steps: Vec<_> = batch::decode(&terms, IBond::getStepsCall { token })?
        .into_iter()
        .map(|s| (s.rangeTo, s.price))
        .collect();
    let supply = batch::decode(&total, IBond::totalSupplyCall {})?;
    if m.reserveToken != reserve
        || m.mintRoyalty > 10000
        || m.burnRoyalty > 10000
        || batch::decode(&issuer, IBond::bondCall {})? != bond
        || batch::decode(&decimals, IBond::decimalsCall {})? != 18
        || steps.first().is_none_or(|s| s.0 == 0)
        || steps.last().is_none_or(|s| U256::from(s.0) < supply)
        || steps
            .windows(2)
            .any(|s| s[0].0 >= s[1].0 || s[0].1 >= s[1].1)
        || ctx.settings.generation() != generation
    {
        return Err(no());
    }
    let available = batch::decode(&balance, IBond::balanceOfCall { account: bond })?;
    Ok(MintClubState {
        bond,
        token,
        reserve,
        supply,
        steps,
        mint_bps: m.mintRoyalty,
        burn_bps: m.burnRoyalty,
        bonded: m.reserveBalance,
        available,
    })
}

fn question(token: Address, qty: U256, mint: bool) -> Bytes {
    if mint {
        IBond::getReserveForTokenCall {
            token,
            tokensToMint: qty,
        }
        .abi_encode()
        .into()
    } else {
        IBond::getRefundForTokensCall {
            token,
            tokensToBurn: qty,
        }
        .abi_encode()
        .into()
    }
}
fn answer(mint: bool, data: &[u8]) -> Option<(U256, U256)> {
    if mint {
        let r = IBond::getReserveForTokenCall::abi_decode_returns_validate(data).ok()?;
        Some((r.reserveAmount, r.royalty))
    } else {
        let r = IBond::getRefundForTokensCall::abi_decode_returns_validate(data).ok()?;
        Some((r.refundAmount, r.royalty))
    }
}
pub(crate) async fn official(
    ctx: &Ctx,
    bond: Address,
    token: Address,
    qty: U256,
    mint: bool,
    at: &Value,
) -> GatewayResult<(U256, U256)> {
    let bad = || {
        ctx.err(ErrorCode::BadResponse,"Mint Club V2: неполный ответ getReserveForToken/getRefundForTokens или отсутствует хеш блока")
    };
    if at["requireCanonical"] != true
        || at["blockHash"]
            .as_str()
            .and_then(|s| s.parse::<B256>().ok())
            .is_none()
    {
        return Err(bad());
    }
    let raw: Bytes = super::super::node::rpc::call(
        ctx,
        "eth_call",
        json!([{"to":bond,"data":question(token,qty,mint)},at]),
    )
    .await?;
    answer(mint, &raw).ok_or_else(bad)
}
fn order(
    e: &super::registry::Entry,
    input: Address,
    amount: U256,
) -> Option<(Address, bool, super::super::math::mint_club::Fill)> {
    let crate::core::types::PoolState::MintClub(state) = &e.pool.state else {
        return None;
    };
    Some((
        state.token,
        input == state.reserve,
        super::super::math::mint_club::fill(state, input, true, amount)?,
    ))
}
pub(crate) fn quote(
    ctx: &Ctx,
    e: &super::registry::Entry,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let (token, mint, fill) = order(e, input, amount)
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Объём недоступен на кривой Mint Club"))?;
    let qty = if mint {
        fill.swapped.amount_out
    } else {
        fill.swapped.amount_in
    };
    Ok((
        ctx.dex_addr("dex.factory", e.family.id)?,
        question(token, qty, mint),
    ))
}
pub(crate) fn payout(
    e: &super::registry::Entry,
    input: Address,
    amount: U256,
    data: &[u8],
) -> Option<U256> {
    let (_, mint, fill) = order(e, input, amount)?;
    let measured = answer(mint, data)?;
    let reserve = if mint {
        fill.swapped.amount_in
    } else {
        fill.swapped.amount_out
    };
    (measured == (reserve, fill.royalty)).then_some(fill.swapped.amount_out)
}

/// Both native events index the issued token, payer and reserve. The shared Bond alone cannot identify a market.
pub(in crate::venues::bsc_dex) fn emitted(
    s: &MintClubState,
    log: &super::super::check::simulate::SimLog,
) -> bool {
    use alloy::sol_types::SolEvent;
    !s.bond.is_zero()
        && log.address == s.bond
        && log.data.len() == 96
        && matches!(log.topics.as_slice(),[action,token,_,reserve] if (*action==IBond::Mint::SIGNATURE_HASH || *action==IBond::Burn::SIGNATURE_HASH) && *token==s.token.into_word() && *reserve==s.reserve.into_word())
}

#[cfg(test)]
#[path = "mint_club_tests.rs"]
mod tests;
