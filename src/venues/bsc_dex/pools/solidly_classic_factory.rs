//! Код фабрики Classic и активная EIP-1967 реализация сверяются один раз на канонический хеш и профиль.
use super::{abi_woo::implementation_slot, families::Family, nami_batch as batch};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::rpc},
};
use alloy_primitives::{Address, B256, Bytes, keccak256};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, LazyLock, Mutex},
};
use tokio::sync::OnceCell;

#[derive(Clone, Copy, Eq, PartialEq)]
struct Binding {
    at: B256,
    address: Address,
    runtime: B256,
    proxy: Option<(Address, B256)>,
}
type Cell = Arc<OnceCell<()>>;
static CACHE: LazyLock<Mutex<VecDeque<(Binding, Cell)>>> = LazyLock::new(Mutex::default);

fn cached(key: Binding) -> Cell {
    let mut cache = CACHE.lock().unwrap_or_else(|err| err.into_inner());
    if let Some((_, hit)) = cache.iter().find(|(k, _)| *k == key) {
        return hit.clone();
    }
    if cache.len() >= 64 {
        cache.pop_front();
    }
    let cell = Cell::default();
    cache.push_back((key, cell.clone()));
    cell
}

fn optional(ctx: &Ctx, key: &str, id: &str) -> GatewayResult<Vec<String>> {
    match ctx.dex_list(key, id) {
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(Vec::new()),
        v => v,
    }
}

async fn verify(ctx: &Ctx, role: Binding, at: &Value) -> GatewayResult<()> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Solidly Classic: фактический код или реализация фабрики изменились",
        )
    };
    let [code, slot] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([role.address, at])),
            (
                "eth_getStorageAt",
                json!([role.address, implementation_slot(), at]),
            ),
        ],
    )
    .await?;
    let expected = role.proxy.map(|(a, _)| a).unwrap_or_default();
    if code.is_empty()
        || keccak256(&code) != role.runtime
        || slot.as_ref() != expected.into_word().as_slice()
    {
        return Err(bad());
    }
    if let Some((implementation, hash)) = role.proxy {
        let code: Bytes = rpc::call(ctx, "eth_getCode", json!([implementation, at])).await?;
        if code.is_empty() || keccak256(&code) != hash {
            return Err(bad());
        }
    }
    Ok(())
}

pub(super) async fn at(ctx: &Ctx, f: &Family, tag: &Value) -> GatewayResult<Address> {
    let bad = || {
        ctx.err(
            ErrorCode::NotConfigured,
            "Solidly Classic: нужны проверенные роли фабрики и хеш канонического блока",
        )
    };
    let address = ctx.dex_addr("dex.factory", f.id)?;
    let runtime = ctx
        .dex_text("dex.factory_runtime_hash", f.id)?
        .parse::<B256>()
        .map_err(|_| bad())?;
    let ips = optional(ctx, "dex.proxy_implementations", f.id)?;
    let hashes = optional(ctx, "dex.proxy_runtime_hashes", f.id)?;
    let proxy = match (ips.as_slice(), hashes.as_slice()) {
        ([], []) => None,
        ([one], [hash]) => {
            let ip = one.parse::<Address>().map_err(|_| bad())?;
            let hash = hash.parse::<B256>().map_err(|_| bad())?;
            if ip.is_zero() || ip == address || hash.is_zero() || hash == keccak256([]) {
                return Err(bad());
            }
            Some((ip, hash))
        }
        _ => return Err(bad()),
    };
    bind(ctx, address, runtime, proxy, tag).await?;
    Ok(address)
}

pub(super) async fn plain(
    ctx: &Ctx,
    address: Address,
    runtime: B256,
    tag: &Value,
) -> GatewayResult<()> {
    bind(ctx, address, runtime, None, tag).await
}

pub(super) async fn bind(
    ctx: &Ctx,
    address: Address,
    runtime: B256,
    proxy: Option<(Address, B256)>,
    tag: &Value,
) -> GatewayResult<()> {
    let bad = || {
        ctx.err(
            ErrorCode::NotConfigured,
            "Нужны проверенный адрес, хеш runtime и канонический блок",
        )
    };
    let at = tag["blockHash"]
        .as_str()
        .and_then(|x| x.parse().ok())
        .ok_or_else(bad)?;
    if address.is_zero()
        || runtime.is_zero()
        || runtime == keccak256([])
        || tag["requireCanonical"] != true
    {
        return Err(bad());
    }
    let key = Binding {
        at,
        address,
        runtime,
        proxy,
    };
    cached(key)
        .get_or_try_init(|| verify(ctx, key, tag))
        .await?;
    Ok(())
}
