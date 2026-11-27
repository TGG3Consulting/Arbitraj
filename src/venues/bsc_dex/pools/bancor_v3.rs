//! Bancor Network v3. All registry, proxy, collection and cash reads use one canonical block hash.
use super::{
    families::Family,
    nami_batch as batch,
    solidly_classic_factory::{bind, plain},
};
use crate::{
    core::types::{
        PoolRef,
        bancor_v3::{BancorMarket, BancorState},
    },
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{abi::IERC20, rpc},
    },
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::{Value, json};
use std::collections::HashMap;

sol! {
    interface IBancor {
        event PoolAdded(address indexed pool,address indexed collection);
        event TokensTraded(bytes32 indexed contextId,address indexed sourceToken,address indexed targetToken,uint256 sourceAmount,uint256 targetAmount,uint256 bntAmount,uint256 targetFeeAmount,uint256 bntFeeAmount,address trader);
        function network() external view returns(address);
        function bnt() external view returns(address);
        function masterVault() external view returns(address);
        function bntPool() external view returns(address);
        function networkSettings() external view returns(address);
        function paused() external view returns(bool);
        function isWhitelisted(address caller) external view returns(bool);
        function liquidityPools() external view returns(address[]);
        function collectionByPool(address pool) external view returns(address);
        function poolLiquidity(address pool) external view returns(uint128 bntTradingLiquidity,uint128 baseTokenTradingLiquidity,uint256 stakedBalance);
        function tradingFeePPM(address pool) external view returns(uint32);
        function networkFeePPM() external view returns(uint32);
        function tradingEnabled(address pool) external view returns(bool);
        function tradeOutputBySourceAmount(address source,address target,uint256 amount) external view returns(uint256);
        function tradeInputByTargetAmount(address source,address target,uint256 amount) external view returns(uint256);
    }
}
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Bancor V3: роли, реестр или физические токены не совпали на выбранном блоке",
    )
}
use super::super::exec::carbon_binding::Code;
pub(crate) fn role(ctx: &Ctx, f: &Family, address: Address) -> GatewayResult<Code> {
    role_map(ctx, &ctx.dex_list("dex.bancor_roles", f.id)?)?
        .get(&address)
        .copied()
        .ok_or_else(|| bad(ctx))
}
fn role_map(ctx: &Ctx, lines: &[String]) -> GatewayResult<HashMap<Address, Code>> {
    let mut pins = HashMap::new();
    for line in lines {
        let fields: Vec<_> = line.split_whitespace().collect();
        let [a, h, i, ih] = fields.as_slice() else {
            return Err(bad(ctx));
        };
        let a = a.parse::<Address>().map_err(|_| bad(ctx))?;
        let ip = i.parse::<Address>().map_err(|_| bad(ctx))?;
        let h = h.parse::<B256>().map_err(|_| bad(ctx))?;
        let ih = ih.parse::<B256>().map_err(|_| bad(ctx))?;
        if a == ip
            || ip.is_zero()
            || pins
                .insert(
                    a,
                    Code {
                        shell: h,
                        implementation: ip,
                        runtime: ih,
                    },
                )
                .is_some()
        {
            return Err(bad(ctx));
        }
    }
    Ok(pins)
}
pub(super) struct Roles {
    pub network: Address,
    pub info: Address,
    pub vault: Address,
    pub bnt: Address,
    pub bnt_decimals: u8,
    pub wrapped: Address,
    pub native: Address,
    collections: HashMap<Address, B256>,
    paused: bool,
    tokens: std::sync::Mutex<HashMap<Address, TokenCell>>,
}
#[derive(PartialEq, Eq)]
struct SnapshotKey {
    block: B256,
    addresses: [Address; 5],
    roles: Vec<String>,
    collections: Vec<String>,
}
type Snapshot = std::sync::Arc<tokio::sync::OnceCell<std::sync::Arc<Roles>>>;
type Snapshots = std::sync::Mutex<std::collections::VecDeque<(SnapshotKey, Snapshot)>>;
static SNAPSHOTS: std::sync::LazyLock<Snapshots> = std::sync::LazyLock::new(Default::default);
pub(super) async fn roles(
    ctx: &Ctx,
    f: &Family,
    at: &Value,
) -> GatewayResult<std::sync::Arc<Roles>> {
    let block = at["blockHash"]
        .as_str()
        .and_then(|v| v.parse::<B256>().ok())
        .filter(|_| at["requireCanonical"] == true)
        .ok_or_else(|| bad(ctx))?;
    let addresses = [
        ctx.dex_addr("dex.factory", f.id)?,
        ctx.dex_addr("dex.quoter", f.id)?,
        ctx.dex_addr("dex.vault", f.id)?,
        ctx.addr("dex.native_wrapped")?,
        ctx.dex_addr("dex.bancor_native_token", f.id)?,
    ];
    let key = SnapshotKey {
        block,
        addresses,
        roles: ctx.dex_list("dex.bancor_roles", f.id)?,
        collections: ctx.dex_list("dex.bancor_collections", f.id)?,
    };
    let configs = (key.roles.clone(), key.collections.clone());
    let cell = {
        let mut cache = SNAPSHOTS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, c)) = cache.iter().find(|(k, _)| *k == key) {
            c.clone()
        } else {
            if cache.len() >= 16 {
                cache.pop_front();
            }
            let c = Snapshot::default();
            cache.push_back((key, c.clone()));
            c
        }
    };
    cell.get_or_try_init(|| read_roles(ctx, addresses, &configs.0, &configs.1, at))
        .await
        .cloned()
}
async fn read_roles(
    ctx: &Ctx,
    addresses: [Address; 5],
    role_lines: &[String],
    collection_lines: &[String],
    at: &Value,
) -> GatewayResult<std::sync::Arc<Roles>> {
    let [network, info, vault, wrapped, native] = addresses;
    let [
        net,
        master,
        bnt,
        bntpool,
        settings,
        net_pause,
        vault_pause,
        native_code,
        wrapped_code,
    ] = batch::read(
        ctx,
        [
            batch::view(at, info, IBancor::networkCall {}),
            batch::view(at, info, IBancor::masterVaultCall {}),
            batch::view(at, info, IBancor::bntCall {}),
            batch::view(at, info, IBancor::bntPoolCall {}),
            batch::view(at, info, IBancor::networkSettingsCall {}),
            batch::view(at, network, IBancor::pausedCall {}),
            batch::view(at, vault, IBancor::pausedCall {}),
            ("eth_getCode", json!([native, at])),
            ("eth_getCode", json!([wrapped, at])),
        ],
    )
    .await?;
    let bnt = batch::decode(&bnt, IBancor::bntCall {})?;
    let bntpool = batch::decode(&bntpool, IBancor::bntPoolCall {})?;
    let settings = batch::decode(&settings, IBancor::networkSettingsCall {})?;
    let addresses = [network, info, vault, bntpool, settings];
    if !native_code.is_empty()
        || wrapped_code.is_empty()
        || batch::decode(&net, IBancor::networkCall {})? != network
        || batch::decode(&master, IBancor::masterVaultCall {})? != vault
        || addresses
            .iter()
            .enumerate()
            .any(|(i, a)| a.is_zero() || addresses[..i].contains(a))
        || [wrapped, native, bnt].iter().enumerate().any(|(i, a)| {
            a.is_zero() || addresses.contains(a) || [wrapped, native, bnt][..i].contains(a)
        })
    {
        return Err(bad(ctx));
    }
    let pins = role_map(ctx, role_lines)?;
    let verify = |a| {
        let p = pins.get(&a).copied();
        async move {
            let code = p.ok_or_else(|| bad(ctx))?;
            bind(
                ctx,
                a,
                code.shell,
                Some((code.implementation, code.runtime)),
                at,
            )
            .await
        }
    };
    tokio::try_join!(
        verify(network),
        verify(info),
        verify(vault),
        verify(bntpool),
        verify(settings)
    )?;
    let mut collections = HashMap::new();
    for line in collection_lines {
        let fields: Vec<_> = line.split_whitespace().collect();
        let [a, h] = fields.as_slice() else {
            return Err(bad(ctx));
        };
        let a = a.parse::<Address>().map_err(|_| bad(ctx))?;
        let h = h.parse::<B256>().map_err(|_| bad(ctx))?;
        if a.is_zero() || h.is_zero() || collections.insert(a, h).is_some() {
            return Err(bad(ctx));
        }
    }
    let [decimals] = batch::read(ctx, [batch::view(at, bnt, IERC20::decimalsCall {})]).await?;
    let bnt_decimals = batch::decode(&decimals, IERC20::decimalsCall {})?;
    Ok(std::sync::Arc::new(Roles {
        bnt_decimals,
        network,
        info,
        vault,
        bnt,
        wrapped,
        native,
        collections,
        paused: batch::decode(&net_pause, IBancor::pausedCall {})?
            || batch::decode(&vault_pause, IBancor::pausedCall {})?,
        tokens: Default::default(),
    }))
}
type TokenCell = std::sync::Arc<tokio::sync::OnceCell<(Option<BancorMarket>, U256)>>;
async fn token(
    ctx: &Ctx,
    r: &Roles,
    pool: Address,
    at: &Value,
) -> GatewayResult<(Option<BancorMarket>, U256)> {
    let cell = {
        let mut tokens = r.tokens.lock().unwrap_or_else(|e| e.into_inner());
        if tokens.len() >= 1024 && !tokens.contains_key(&pool) {
            TokenCell::default()
        } else {
            tokens.entry(pool).or_default().clone()
        }
    };
    cell.get_or_try_init(|| async {
        tokio::try_join!(market(ctx, r, pool, at), balance(ctx, r, pool, at))
    })
    .await
    .cloned()
}
async fn market(
    ctx: &Ctx,
    r: &Roles,
    pool: Address,
    at: &Value,
) -> GatewayResult<Option<BancorMarket>> {
    if pool == r.bnt {
        return Ok(None);
    }
    let lookup = IBancor::collectionByPoolCall { pool };
    let [out] = batch::read(ctx, [batch::view(at, r.network, lookup.clone())]).await?;
    let col = batch::decode(&out, lookup)?;
    let pin = *r.collections.get(&col).ok_or_else(|| bad(ctx))?;
    let liq = IBancor::poolLiquidityCall { pool };
    let fee = IBancor::tradingFeePPMCall { pool };
    let on = IBancor::tradingEnabledCall { pool };
    let read = batch::read(
        ctx,
        [
            batch::view(at, col, liq.clone()),
            batch::view(at, col, fee.clone()),
            batch::view(at, col, on.clone()),
            batch::view(at, col, IBancor::networkFeePPMCall {}),
        ],
    );
    let ((), [l, f, e, n]) = tokio::try_join!(plain(ctx, col, pin, at), read)?;
    let l = batch::decode(&l, liq)?;
    Ok(Some(BancorMarket {
        token: pool,
        collection: col,
        bnt: U256::from(l.bntTradingLiquidity),
        base: U256::from(l.baseTokenTradingLiquidity),
        staked: l.stakedBalance,
        fee_ppm: batch::decode(&f, fee)?,
        network_fee_ppm: batch::decode(&n, IBancor::networkFeePPMCall {})?,
        enabled: batch::decode(&e, on)? && !r.paused,
    }))
}
async fn balance(ctx: &Ctx, r: &Roles, t: Address, at: &Value) -> GatewayResult<U256> {
    if t == r.native {
        return rpc::call(ctx, "eth_getBalance", json!([r.vault, at])).await;
    }
    let c = IERC20::balanceOfCall { owner: r.vault };
    let [b] = batch::read(ctx, [batch::view(at, t, c.clone())]).await?;
    batch::decode(&b, c)
}
pub(crate) async fn load(
    ctx: &Ctx,
    f: &Family,
    reference: &PoolRef,
    tokens: [Address; 2],
    at: &Value,
) -> GatewayResult<BancorState> {
    let generation = ctx.settings.generation();
    let r = roles(ctx, f, at).await?;
    let PoolRef::Multi {
        contract,
        token0,
        token1,
    } = reference
    else {
        return Err(bad(ctx));
    };
    if *contract != r.network || token0 >= token1 || token0.is_zero() || tokens[0] >= tokens[1] {
        return Err(bad(ctx));
    }
    let norm = |t| if t == r.native { r.wrapped } else { t };
    let mut physical = [*token0, *token1];
    if norm(physical[0]) > norm(physical[1]) {
        physical.swap(0, 1);
    }
    if physical.map(norm) != tokens {
        return Err(bad(ctx));
    }
    let ((a, x), (b, y)) = tokio::try_join!(
        token(ctx, &r, physical[0], at),
        token(ctx, &r, physical[1], at)
    )?;
    if generation != ctx.settings.generation() {
        return Err(bad(ctx));
    }
    Ok(BancorState {
        network: r.network,
        info: r.info,
        vault: r.vault,
        bnt: r.bnt,
        bnt_decimals: r.bnt_decimals,
        wrapped: r.wrapped,
        tokens,
        physical,
        markets: [a, b],
        vault_balances: [x, y],
    })
}
pub(crate) async fn arb(
    ctx: &Ctx,
    s: &BancorState,
    actual_sender: Address,
    at: &Value,
) -> GatewayResult<bool> {
    if actual_sender.is_zero() || at["requireCanonical"] != true || !at["blockHash"].is_string() {
        return Ok(false);
    }
    let c = IBancor::isWhitelistedCall {
        caller: actual_sender,
    };
    let [r] = batch::read(ctx, [batch::view(at, s.network, c.clone())]).await?;
    batch::decode(&r, c)
}
pub(crate) fn request(
    ctx: &Ctx,
    s: &BancorState,
    input: Address,
    exact_in: bool,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let i = s.side(input).ok_or_else(|| bad(ctx))?;
    let (source, target) = (s.physical[i], s.physical[1 - i]);
    let data = if exact_in {
        IBancor::tradeOutputBySourceAmountCall {
            source,
            target,
            amount,
        }
        .abi_encode()
    } else {
        IBancor::tradeInputByTargetAmountCall {
            source,
            target,
            amount,
        }
        .abi_encode()
    };
    Ok((s.info, data.into()))
}
pub(crate) async fn official(
    ctx: &Ctx,
    s: &BancorState,
    input: Address,
    exact_in: bool,
    amount: U256,
    at: &Value,
) -> GatewayResult<U256> {
    let (to, data) = request(ctx, s, input, exact_in, amount)?;
    let out: Bytes = rpc::call(ctx, "eth_call", json!([{"to":to,"data":data},at])).await?;
    decode(&out).ok_or_else(|| bad(ctx))
}
pub(crate) fn decode(b: &[u8]) -> Option<U256> {
    (b.len() == 32)
        .then(|| U256::from_be_slice(b))
        .filter(|n| !n.is_zero())
}
