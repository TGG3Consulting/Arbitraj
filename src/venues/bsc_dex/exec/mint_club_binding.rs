//! Аттестация собственного модуля Mint Club и разрешённых реализаций резерва на том же каноническом блоке.
use super::super::{
    ctx::Ctx,
    node::rpc,
    pools::{direct_module, registry::Entry},
    quote::white_state::Head,
};
use super::mint_club_wire::{IBondFlow, Protection};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::json;

pub(in crate::venues::bsc_dex) async fn nominal(
    ctx: &Ctx,
    e: &Entry,
    reserve: Address,
    h: Head,
) -> GatewayResult<()> {
    if ctx.addr("dex.native_wrapped")? == reserve {
        let raw = ctx.dex_list("dex.mint_nominal_reserve_hashes", e.family.id)?;
        let allowed: Option<Vec<B256>> = raw.iter().map(|s| s.parse().ok()).collect();
        if let Some(allowed) = allowed
            && !allowed.is_empty()
        {
            let code: Bytes = rpc::call(ctx, "eth_getCode", json!([reserve, h.selector()])).await?;
            if direct_module::matching_hash(&allowed, &code).is_some() {
                return Ok(());
            }
        }
    }
    Err(ctx.err(
        ErrorCode::NoRoute,
        "Mint Club: защита номинального ERC20 перевода этой реализации не подтверждена",
    ))
}

pub(in crate::venues::bsc_dex) async fn module(
    ctx: &Ctx,
    e: &Entry,
    selected: Address,
    h: Head,
) -> GatewayResult<Protection> {
    let id = e.family.id;
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Mint Club: адрес, код или неизменяемые реквизиты выбранного модуля изменились",
        )
    };
    let generation = ctx.settings.generation();
    let module = ctx.dex_addr("dex.router", id)?;
    if module != selected || module.is_zero() {
        return Err(fail());
    }
    let bond = ctx.dex_addr("dex.factory", id)?;
    let allowed = direct_module::configured(ctx, id)?;
    let templates = ctx.dex_list("dex.proxy_implementations", id)?;
    let [template] = templates.as_slice() else {
        return Err(fail());
    };
    let template = template.parse::<Address>().map_err(|_| fail())?;
    let mut clone = alloy_primitives::hex!("363d3d373d3d3d363d73").to_vec();
    clone.extend_from_slice(template.as_slice());
    clone.extend_from_slice(&alloy_primitives::hex!("5af43d82803e903d91602b57fd5bf3"));
    let at = h.selector();
    let view = |data: Vec<u8>| json!([{"to":module,"data":Bytes::from(data)},at]);
    let (code, b, c) = tokio::try_join!(
        rpc::call::<Bytes>(ctx, "eth_getCode", json!([module, at])),
        rpc::call::<Bytes>(ctx, "eth_call", view(IBondFlow::bondCall {}.abi_encode())),
        rpc::call::<Bytes>(
            ctx,
            "eth_call",
            view(IBondFlow::cloneHashCall {}.abi_encode())
        )
    )?;
    if direct_module::matching_hash(&allowed, &code).is_none()
        || IBondFlow::bondCall::abi_decode_returns_validate(&b).ok() != Some(bond)
        || IBondFlow::cloneHashCall::abi_decode_returns_validate(&c).ok() != Some(keccak256(clone))
        || ctx.settings.generation() != generation
    {
        return Err(fail());
    }
    Ok(Protection {
        module,
        bond,
        deadline: U256::from(
            h.timestamp
                .to::<u64>()
                .saturating_add(ctx.int("dex.deadline_s")?),
        ),
    })
}
