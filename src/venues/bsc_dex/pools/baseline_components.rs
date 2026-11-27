//! Диспетчер Baseline Mercury делегирует ABI-селекторы семи компонентам на контрольном снимке.
//! Сверяет весь текущий реестр и код компонентов на одной высоте. Получатель, валюта резерва
//! и физический PoolId проверяются отдельно для каждого рынка, включая новые Initialize.
use super::{
    launch_keys::{Role, invalid},
    nami_batch as batch,
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        ctx::Ctx,
        node::{multicall, rpc},
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, keccak256};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::RwLock};

alloy::sol! { interface IMercury {
    function getComponents() external view returns(address[]);
    function ROUTES() external view returns(bytes4[]);
    function routes(bytes4 selector) external view returns(address);
    function poolIdToBToken(bytes32 id) external view returns(address);
    function reserve(address token) external view returns(address);
    function hasHook(address token) external view returns(bool);
    function isPoolPaused(address token) external view returns(bool);
    function isProtocolPaused() external view returns(bool);
}}

#[derive(PartialEq, Eq)]
struct Binding {
    block: B256,
    relay: Address,
    components: Vec<(Address, B256)>,
}
static VERIFIED: RwLock<Option<Binding>> = RwLock::new(None);

async fn aggregate(ctx: &Ctx, at: &Value, calls: &[(Address, Bytes)]) -> GatewayResult<Vec<Bytes>> {
    let target = ctx.addr("dex.multicall3")?;
    let raw: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":target,"data":multicall::encode(calls)},at]),
    )
    .await?;
    multicall::decode(&raw, calls.len())?
        .into_iter()
        .map(|r| r.map_err(|_| invalid(ctx)))
        .collect()
}

pub(super) async fn components(
    ctx: &Ctx,
    relay: Address,
    roles: &[Role],
    at: &Value,
) -> GatewayResult<()> {
    let block = at
        .get("blockHash")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid(ctx))?;
    if at["requireCanonical"] != true {
        return Err(invalid(ctx));
    }
    let mut expected: Vec<_> = roles
        .iter()
        .filter(|r| r.kind == "component")
        .map(|r| (r.address, r.hash))
        .collect();
    expected.sort_unstable();
    if expected.is_empty() {
        return Err(invalid(ctx));
    }
    let binding = Binding {
        block,
        relay,
        components: expected,
    };
    if VERIFIED
        .read()
        .ok()
        .is_some_and(|v| v.as_ref() == Some(&binding))
    {
        return Ok(());
    }
    let directory = IMercury::getComponentsCall {};
    let [raw] = batch::read(ctx, [batch::view(at, relay, directory.clone())]).await?;
    let mut listed = batch::decode(&raw, directory)?;
    listed.sort_unstable();
    if listed
        .iter()
        .copied()
        .ne(binding.components.iter().map(|r| r.0))
    {
        return Err(invalid(ctx));
    }
    let code: Vec<_> = listed
        .iter()
        .map(|a| ("eth_getCode", json!([a, at])))
        .collect();
    let implementation = super::abi_woo::implementation_slot();
    let beacon = B256::from(
        (alloy_primitives::U256::from_be_bytes(keccak256("eip1967.proxy.beacon").0)
            - alloy_primitives::U256::from(1))
        .to_be_bytes::<32>(),
    );
    let slots: Vec<_> = listed
        .iter()
        .copied()
        .chain(std::iter::once(relay))
        .flat_map(|a| {
            [implementation, beacon].map(|slot| ("eth_getStorageAt", json!([a, slot, at])))
        })
        .collect();
    let methods = IMercury::ROUTESCall {};
    let queries: Vec<_> = listed
        .iter()
        .map(|a| (*a, Bytes::from(methods.abi_encode())))
        .collect();
    let (raw, values, selectors) = tokio::try_join!(
        rpc::batch(ctx, &code),
        rpc::batch(ctx, &slots),
        aggregate(ctx, at, &queries)
    )?;
    if values.len() != slots.len() {
        return Err(invalid(ctx));
    }
    for v in values {
        if serde_json::from_str::<B256>(v?.get()).ok() != Some(B256::ZERO) {
            return Err(invalid(ctx));
        }
    }
    let mut routes = Vec::new();
    let mut owners = Vec::new();
    let mut seen = BTreeSet::new();
    if raw.len() != binding.components.len() {
        return Err(invalid(ctx));
    }
    for (((component, hash), bytes), advertised) in
        binding.components.iter().zip(raw).zip(selectors)
    {
        let code: Bytes = serde_json::from_str(bytes?.get()).map_err(|_| invalid(ctx))?;
        if code.is_empty() || keccak256(code) != *hash {
            return Err(invalid(ctx));
        }
        for selector in batch::decode(&advertised, methods.clone())? {
            if !seen.insert(selector) {
                return Err(invalid(ctx));
            }
            routes.push((
                relay,
                Bytes::from(IMercury::routesCall { selector }.abi_encode()),
            ));
            owners.push(*component);
        }
    }
    if routes.is_empty() {
        return Err(invalid(ctx));
    }
    let checked = aggregate(ctx, at, &routes).await?;
    for (value, owner) in checked.into_iter().zip(owners) {
        if IMercury::routesCall::abi_decode_returns_validate(&value).ok() != Some(owner) {
            return Err(invalid(ctx));
        }
    }
    if let Ok(mut v) = VERIFIED.write() {
        *v = Some(binding);
    }
    Ok(())
}
