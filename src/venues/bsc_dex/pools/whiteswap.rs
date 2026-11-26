//! Проверка прокси и рынка WhiteSwap по одному каноническому хешу перед использованием резервов.
use super::{
    abi_check::IPoolImmutables,
    abi_v2v3::{IV2Factory, IV2Pair},
    families::Family,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{call, jsonrpc, rpc},
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct Proxy {
    address: Address,
    implementation: Address,
    proxy_hash: B256,
    implementation_hash: B256,
}

fn profile(ctx: &Ctx, family: &Family, index: usize, address: Address) -> GatewayResult<Proxy> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "WhiteSwap: нужны реализации и хеши прокси выбранного пути",
        )
    };
    let ips = ctx.dex_list("dex.proxy_implementations", family.id)?;
    let hashes = ctx.dex_list("dex.proxy_runtime_hashes", family.id)?;
    if ips.len() != 3 || hashes.len() != 6 {
        return Err(bad());
    }
    let implementation = ips[index].parse::<Address>().map_err(|_| bad())?;
    let proxy_hash = hashes[index * 2].parse::<B256>().map_err(|_| bad())?;
    let implementation_hash = hashes[index * 2 + 1].parse::<B256>().map_err(|_| bad())?;
    if address.is_zero()
        || implementation.is_zero()
        || address == implementation
        || proxy_hash.is_zero()
        || implementation_hash.is_zero()
    {
        return Err(bad());
    }
    Ok(Proxy {
        address,
        implementation,
        proxy_hash,
        implementation_hash,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    address: Address,
    code_hash: B256,
    storage_proof: Vec<Storage>,
}
#[derive(Deserialize)]
struct Storage {
    key: U256,
    value: U256,
}

fn account_ok(
    proof: &Account,
    address: Address,
    hash: B256,
    slot: Option<(B256, Address)>,
) -> bool {
    if proof.address != address || proof.code_hash != hash {
        return false;
    }
    match (slot, proof.storage_proof.as_slice()) {
        (None, []) => true,
        (Some((key, target)), [entry]) => {
            entry.key == U256::from_be_bytes(key.0)
                && entry.value == U256::from_be_bytes(target.into_word().0)
        }
        _ => false,
    }
}

// Один и тот же набор прокси часто проверяется на нескольких объёмах. Кэшируем только
// доказанную идентичность кода. Резервы и принадлежность пары всегда читает EIP-1898.
type Admission = std::sync::Arc<tokio::sync::OnceCell<()>>;
type CheckedProxies = std::collections::HashMap<Vec<Proxy>, Admission>;
#[derive(Default)]
struct BlockChecks(std::collections::VecDeque<(B256, CheckedProxies)>);

impl BlockChecks {
    fn cell(&mut self, hash: B256, roles: impl AsRef<[Proxy]>) -> Admission {
        let roles = roles.as_ref();
        let index = match self.0.iter().position(|(h, _)| *h == hash) {
            Some(i) => i,
            None => {
                if self.0.len() == 2 {
                    self.0.pop_front();
                }
                self.0.push_back((hash, CheckedProxies::default()));
                self.0.len() - 1
            }
        };
        let cache = &mut self.0[index].1;
        if let Some(cell) = cache.get(roles) {
            return cell.clone();
        }
        if cache.len() >= 1024 {
            return Admission::default();
        }
        cache.entry(roles.to_vec()).or_default().clone()
    }
}

fn shared_cell(hash: B256, roles: &[Proxy]) -> Admission {
    static CACHE: std::sync::LazyLock<std::sync::Mutex<BlockChecks>> =
        std::sync::LazyLock::new(Default::default);
    CACHE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .cell(hash, roles)
}

async fn implementations(ctx: &Ctx, roles: &[Proxy], at: &Value) -> GatewayResult<()> {
    let slot = super::abi_woo::implementation_slot();
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "WhiteSwap: изменилась реализация EIP-1967 или код прокси/реализации",
        )
    };
    let mut req = Vec::with_capacity(6);
    for r in roles {
        req.push(("eth_getProof", json!([r.address, [slot], at])));
        req.push(("eth_getProof", json!([r.implementation, [], at])));
    }
    // Узел без архивных trie proofs может всё ещё отдать код и хранилище на том же блоке.
    if let Ok(rows) = rpc::batch(ctx, &req).await
        && rows.len() == req.len()
        && let Ok(accounts) = rows
            .into_iter()
            .map(|r| r.and_then(|raw| jsonrpc::decode::<Account>(raw.as_ref(), rpc::SOURCE)))
            .collect::<GatewayResult<Vec<_>>>()
    {
        for (i, r) in roles.iter().enumerate() {
            if !account_ok(
                &accounts[2 * i],
                r.address,
                r.proxy_hash,
                Some((slot, r.implementation)),
            ) || !account_ok(
                &accounts[2 * i + 1],
                r.implementation,
                r.implementation_hash,
                None,
            ) {
                return Err(bad());
            }
        }
        return Ok(());
    }
    req.clear();
    for r in roles {
        req.push(("eth_getStorageAt", json!([r.address, slot, at])));
        req.push(("eth_getCode", json!([r.address, at])));
        req.push(("eth_getCode", json!([r.implementation, at])));
    }
    let rows = rpc::batch(ctx, &req).await?;
    if rows.len() != req.len() {
        return Err(bad());
    }
    let mut rows = rows.into_iter();
    for r in roles {
        let mut next = || rows.next().ok_or_else(bad)?;
        let word: B256 = jsonrpc::decode(next()?.as_ref(), rpc::SOURCE)?;
        let proxy: Bytes = jsonrpc::decode(next()?.as_ref(), rpc::SOURCE)?;
        let implementation: Bytes = jsonrpc::decode(next()?.as_ref(), rpc::SOURCE)?;
        if word != r.implementation.into_word()
            || proxy.is_empty()
            || implementation.is_empty()
            || keccak256(proxy) != r.proxy_hash
            || keccak256(implementation) != r.implementation_hash
        {
            return Err(bad());
        }
    }
    Ok(())
}

fn reward_matches(code: &[u8], role: Proxy, token: Address, start: usize) -> bool {
    !token.is_zero()
        && !code.is_empty()
        && keccak256(code) == role.implementation_hash
        && start.checked_sub(1).and_then(|i| code.get(i)) == Some(&0x73)
        && start.checked_add(20).and_then(|end| code.get(start..end)) == Some(token.as_slice())
}

/// Адрес WSE зашит в исходнике WSRouter. После первой сверки достаточно проверять хеш кода:
/// одно и то же keccak256 всегда обозначает те же байты, включая аргумент PUSH20.
async fn pinned_operand(
    ctx: &Ctx,
    role: Proxy,
    token: Address,
    offset: usize,
    at: &Value,
) -> GatewayResult<()> {
    type Key = (B256, Address, usize);
    static CHECKED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<Key>>> =
        std::sync::LazyLock::new(Default::default);
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "WhiteSwap: адрес в настройках отличается от PUSH20 проверенной реализации",
        )
    };
    let key = (role.implementation_hash, token, offset);
    if CHECKED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains(&key)
    {
        return Ok(());
    }
    let code: Bytes = rpc::call(ctx, "eth_getCode", json!([role.implementation, at])).await?;
    if !reward_matches(&code, role, token, offset) {
        return Err(bad());
    }
    let mut cache = CHECKED.lock().unwrap_or_else(|p| p.into_inner());
    if cache.len() < 128 {
        cache.insert(key);
    }
    Ok(())
}

/// Прямой роутер проверяется независимо от V5, который обращается только к паре.
pub async fn check_router(
    ctx: &Ctx,
    family: &Family,
    pool: Address,
    at: &Value,
) -> GatewayResult<()> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "WhiteSwap: неподтверждённый роутер или некорректное смещение адреса",
        )
    };
    let hash = at["blockHash"]
        .as_str()
        .and_then(|s| s.parse::<B256>().ok())
        .filter(|h| !h.is_zero())
        .ok_or_else(bad)?;
    if at["requireCanonical"] != true {
        return Err(bad());
    }
    let factory = ctx.dex_addr("dex.factory", family.id)?;
    let router = ctx.dex_addr("dex.router", family.id)?;
    let reward = ctx.dex_addr("dex.whiteswap_reward_token", family.id)?;
    let offset = |key| usize::try_from(ctx.dex_int(key, family.id)?).map_err(|_| bad());
    let wse_offset = offset("dex.whiteswap_reward_offset")?;
    let router_offset = offset("dex.whiteswap_router_offset")?;
    let pair = profile(ctx, family, 2, pool)?;
    let roles = [profile(ctx, family, 1, router)?];
    let cell = shared_cell(hash, &roles);
    let (_, _, _, actual) = tokio::try_join!(
        cell.get_or_try_init(|| implementations(ctx, &roles, at)),
        pinned_operand(ctx, roles[0], reward, wse_offset, at),
        pinned_operand(ctx, pair, router, router_offset, at),
        super::pmm_view::view(
            ctx,
            at,
            Address::ZERO,
            router,
            IPoolImmutables::factoryCall {}
        )
    )?;
    if actual != factory {
        return Err(bad());
    }
    Ok(())
}

pub async fn check(
    ctx: &Ctx,
    family: &Family,
    pool: Address,
    tokens: (Address, Address),
    at: &Value,
) -> GatewayResult<(U256, U256)> {
    let bad = || {
        ctx.err(ErrorCode::NotApplicable,"WhiteSwap: рынок не соответствует фабрике, роутеру или подтверждённому коду создания пары")
    };
    let block_hash = at["blockHash"]
        .as_str()
        .and_then(|s| s.parse::<B256>().ok())
        .filter(|h| !h.is_zero())
        .ok_or_else(bad)?;
    if at["requireCanonical"] != true {
        return Err(bad());
    }
    let factory = ctx.dex_addr("dex.factory", family.id)?;
    let roles = [
        profile(ctx, family, 0, factory)?,
        profile(ctx, family, 2, pool)?,
    ];
    let init = ctx.dex_list("dex.pool_code_hashes", family.id)?;
    let [value] = init.as_slice() else {
        return Err(bad());
    };
    let hash = value.parse::<B256>().map_err(|_| bad())?;
    let (a, b) = tokens;
    if a.is_zero()
        || b.is_zero()
        || a == b
        || hash.is_zero()
        || super::verify::v2_create2(factory, a, b, hash) != pool
        || ctx.dex_dec("dex.v2_fee_pct", family.id)? != rust_decimal::Decimal::new(3, 1)
        || ctx.dex_text("dex.v2_fee_source", family.id)? != "fixed"
    {
        return Err(bad());
    }
    let req = [
        (
            factory,
            IV2Factory::getPairCall {
                tokenA: a,
                tokenB: b,
            }
            .abi_encode(),
        ),
        (pool, IPoolImmutables::factoryCall {}.abi_encode()),
        (pool, IV2Pair::getReservesCall {}.abi_encode()),
    ]
    .map(|(address, data)| {
        (
            "eth_call",
            json!([{"to":address,"data":Bytes::from(data)},at]),
        )
    });
    let cell = shared_cell(block_hash, &roles);
    let (_, rows) = tokio::try_join!(
        cell.get_or_try_init(|| implementations(ctx, &roles, at)),
        rpc::batch(ctx, &req)
    )?;
    let mut rows = rows.into_iter();
    let mut next = || -> GatewayResult<Bytes> {
        let r = rows.next().ok_or_else(bad)??;
        jsonrpc::decode(&r, rpc::SOURCE)
    };
    for want in [pool, factory] {
        if call::decode::<IPoolImmutables::factoryCall>(&next()?)? != want {
            return Err(bad());
        }
    }
    let r = call::decode::<IV2Pair::getReservesCall>(&next()?)?;
    Ok((U256::from(r.reserve0), U256::from(r.reserve1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reward_uses_the_accepted_implementation_and_token_operand() {
        let token = Address::repeat_byte(5);
        let mut code = vec![0x73];
        code.extend_from_slice(token.as_slice());
        let role = Proxy {
            address: Address::repeat_byte(1),
            implementation: Address::repeat_byte(2),
            proxy_hash: B256::ZERO,
            implementation_hash: keccak256(&code),
        };
        assert!(reward_matches(&code, role, token, 1));
        assert!(!reward_matches(&code, role, Address::repeat_byte(4), 1));
        assert!(!reward_matches(&code, role, token, usize::MAX));
    }

    #[test]
    fn quote_sizes_share_only_the_same_block_and_accepted_roles() {
        use std::sync::Arc;
        let proxy = Proxy {
            address: Address::repeat_byte(1),
            implementation: Address::repeat_byte(2),
            proxy_hash: B256::repeat_byte(3),
            implementation_hash: B256::repeat_byte(4),
        };
        let mut cache = BlockChecks::default();
        let h = B256::repeat_byte(5);
        let first = cache.cell(h, [proxy; 3]);
        assert!(Arc::ptr_eq(&first, &cache.cell(h, [proxy; 3])));
        assert!(!Arc::ptr_eq(
            &first,
            &cache.cell(
                h,
                [Proxy {
                    proxy_hash: B256::ZERO,
                    ..proxy
                }; 3]
            )
        ));
        assert!(!Arc::ptr_eq(
            &first,
            &cache.cell(B256::repeat_byte(6), [proxy; 3])
        ));
        cache.cell(B256::repeat_byte(7), [proxy; 3]);
        assert!(!Arc::ptr_eq(&first, &cache.cell(h, [proxy; 3])));
    }
    #[test]
    fn different_implementation_or_slot_fails_even_when_proxy_hash_matches() {
        let address = Address::repeat_byte(1);
        let implementation = Address::repeat_byte(2);
        let code = B256::repeat_byte(3);
        let slot = super::super::abi_woo::implementation_slot();
        let proof = Account {
            address,
            code_hash: code,
            storage_proof: vec![Storage {
                key: U256::from_be_bytes(slot.0),
                value: U256::from_be_bytes(implementation.into_word().0),
            }],
        };
        assert!(account_ok(
            &proof,
            address,
            code,
            Some((slot, implementation))
        ));
        assert!(!account_ok(&proof, address, code, Some((slot, address))));
        assert!(!account_ok(
            &proof,
            address,
            code,
            Some((B256::ZERO, implementation))
        ));
        assert!(!account_ok(
            &proof,
            address,
            B256::ZERO,
            Some((slot, implementation))
        ));
    }
}
