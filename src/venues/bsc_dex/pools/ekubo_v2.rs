//! Ethereum Ekubo V2 packs price in the low 96 bits and uses standard Solidity mappings.
//! The official V2 Router multicall enforces a full input and the credited output atomically.
use alloy::sol;
use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::{Address, B256, Bytes, U256, aliases::U96, keccak256};
use serde_json::{Value, json};

use super::super::{ctx::Ctx, node::rpc};
use super::families::Family;
use crate::gateway::errors::{ErrorCode, GatewayResult};

sol! {
    struct EkuboPoolKey {address token0;address token1;bytes32 config;}
    struct BoundsV2 { int32 lower; int32 upper; }
    struct PositionV2 { bytes32 salt; BoundsV2 bounds; int128 liquidityDelta; }
    struct NodeV2 { EkuboPoolKey poolKey; uint96 sqrtRatioLimit; uint256 skipAhead; }
    struct TokenV2 { address token; int128 amount; }
    struct OrderV2 { NodeV2[] route; TokenV2 tokenAmount; }
    interface IRouterV2 {
        event PositionUpdated(address locker,bytes32 poolId,PositionV2 params,int128 delta0,int128 delta1);
        function quote(EkuboPoolKey poolKey,bool isToken1,int128 amount,uint96 sqrtRatioLimit,uint256 skipAhead) external returns(int128 delta0,int128 delta1);
        function multihopSwap(OrderV2 order,int256 threshold) external payable;
        function recordBalanceForSlippageCheck(address token) external payable;
        function checkMinimumOutputReceived(address token,uint256 minimum) external payable;
        function checkDeadline(uint256 deadline) external payable;
        function multicall(bytes[] data) external payable;
    }
}

pub fn position_topic() -> B256 {
    IRouterV2::PositionUpdated::SIGNATURE_HASH
}

pub fn price_limit(config: B256, words: (u64, u64), side: bool) -> Option<U96> {
    use super::super::math::{
        ekubo_bitmap::tick_of,
        ekubo_tick::{MAX_TICK, MIN_TICK, to_sqrt_ratio_at},
    };
    let spacing = u32::from_be_bytes(config[28..].try_into().ok()?);
    let tick = if spacing == 0 {
        if side { MAX_TICK } else { MIN_TICK }
    } else if side {
        tick_of(words.1, 255, spacing).min(i64::from(MAX_TICK)) as i32
    } else {
        tick_of(words.0, 0, spacing).max(i64::from(MIN_TICK)) as i32
    };
    Some(U96::from(to_sqrt_ratio_at(tick)?))
}

fn mapping(key: B256, base: B256) -> B256 {
    let mut value = [0u8; 64];
    value[..32].copy_from_slice(key.as_slice());
    value[32..].copy_from_slice(base.as_slice());
    keccak256(value)
}
pub fn state_slot(id: B256) -> B256 {
    mapping(id, B256::from(U256::from(2).to_be_bytes::<32>()))
}
pub fn bitmap_slot(id: B256, index: u64) -> B256 {
    mapping(
        B256::from(U256::from(index).to_be_bytes::<32>()),
        mapping(id, B256::from(U256::from(7).to_be_bytes::<32>())),
    )
}
pub fn tick_slot(id: B256, tick: i32) -> B256 {
    let mut key = [if tick < 0 { 0xff } else { 0 }; 32];
    key[28..].copy_from_slice(&tick.to_be_bytes());
    mapping(
        B256::from(key),
        mapping(id, B256::from(U256::from(5).to_be_bytes::<32>())),
    )
}

pub fn decode_state(value: &B256) -> (U256, i32, u128) {
    (
        U256::from_be_slice(&value[20..]),
        i32::from_be_bytes(value[16..20].try_into().expect("four bytes")),
        u128::from_be_bytes(value[..16].try_into().expect("sixteen bytes")),
    )
}

/// V2 uses a plain uint32 tick spacing. Normalise only the internal V3 math representation.
pub fn math_config(mut config: B256) -> Option<B256> {
    let spacing = u32::from_be_bytes(config[28..].try_into().ok()?);
    if !config[..20].iter().all(|&a| a == 0) || spacing > 698605 {
        return None;
    }
    if spacing > 0 {
        config[28] |= 0x80;
    }
    Some(config)
}

pub fn raw_config(mut math: B256) -> B256 {
    math[28] &= 0x7f;
    math
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct OrderKey {
    pub token0: Address,
    pub token1: Address,
    pub config: B256,
    pub side: bool,
    pub amount: i128,
    pub price_limit: U96,
}
impl OrderKey {
    fn pool(self) -> EkuboPoolKey {
        EkuboPoolKey {
            token0: self.token0,
            token1: self.token1,
            config: self.config,
        }
    }
    fn assets(self) -> (Address, Address) {
        if self.side {
            (self.token1, self.token0)
        } else {
            (self.token0, self.token1)
        }
    }
    pub fn calldata(self, minimum: U256, deadline: U256) -> Bytes {
        let (input, output) = self.assets();
        let node = NodeV2 {
            poolKey: self.pool(),
            sqrtRatioLimit: self.price_limit,
            skipAhead: U256::ZERO,
        };
        let order = OrderV2 {
            route: vec![node],
            tokenAmount: TokenV2 {
                token: input,
                amount: self.amount,
            },
        };
        let calls = vec![
            Bytes::from(IRouterV2::checkDeadlineCall { deadline }.abi_encode()),
            Bytes::from(
                IRouterV2::recordBalanceForSlippageCheckCall { token: output }.abi_encode(),
            ),
            Bytes::from(
                IRouterV2::multihopSwapCall {
                    order,
                    threshold: Default::default(),
                }
                .abi_encode(),
            ),
            Bytes::from(
                IRouterV2::checkMinimumOutputReceivedCall {
                    token: output,
                    minimum,
                }
                .abi_encode(),
            ),
        ];
        IRouterV2::multicallCall { data: calls }.abi_encode().into()
    }
    pub fn value(self) -> U256 {
        if self.assets().0.is_zero() {
            U256::from(self.amount.unsigned_abs())
        } else {
            U256::ZERO
        }
    }
    pub fn quote(self) -> Bytes {
        IRouterV2::quoteCall {
            poolKey: self.pool(),
            isToken1: self.side,
            amount: self.amount,
            sqrtRatioLimit: self.price_limit,
            skipAhead: U256::ZERO,
        }
        .abi_encode()
        .into()
    }
    pub fn payout(self, out: &[u8]) -> Option<U256> {
        let r = IRouterV2::quoteCall::abi_decode_returns_validate(out).ok()?;
        let (paid, got) = if self.side {
            (r.delta1, r.delta0)
        } else {
            (r.delta0, r.delta1)
        };
        (paid == self.amount && paid > 0 && got < 0).then(|| U256::from(got.unsigned_abs()))
    }
}

pub async fn check(ctx: &Ctx, f: &Family, core: Address, tag: &Value) -> GatewayResult<Address> {
    let no = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "Ekubo V2: код роутера или указанное ядро не совпадают с настройками",
        )
    };
    if ctx.dex_addr("dex.ekubo_core", f.id)? != core {
        return Err(no());
    }
    let router = ctx.dex_addr("dex.quoter", f.id)?;
    let expected = |name| ctx.dex_text(name, f.id)?.parse::<B256>().map_err(|_| no());
    let core_hash = expected("dex.factory_runtime_hash")?;
    let router_hash = expected("dex.router_runtime_hash")?;
    let (a, b) = tokio::try_join!(
        rpc::call::<Bytes>(ctx, "eth_getCode", json!([core, tag])),
        rpc::call::<Bytes>(ctx, "eth_getCode", json!([router, tag]))
    )?;
    // In the verified router all nine immutable address PUSH32 operands refer to Core.
    if a.is_empty()
        || keccak256(&a) != core_hash
        || keccak256(&b) != router_hash
        || [1369, 2044, 2979, 6490, 7103, 9996, 10923, 11045, 11147]
            .iter()
            .any(|&i| b.get(i..i + 20) != Some(core.as_slice()))
    {
        return Err(no());
    }
    Ok(router)
}

/// Canonical pool snapshot. One batch reads each sparse set of regular storage slots.
async fn slots(ctx: &Ctx, core: Address, keys: &[B256], tag: &Value) -> GatewayResult<Vec<B256>> {
    let mut result = Vec::with_capacity(keys.len());
    for chunk in keys.chunks(64) {
        let requests: Vec<_> = chunk
            .iter()
            .map(|k| ("eth_getStorageAt", json!([core, k, tag])))
            .collect();
        let raw = rpc::batch(ctx, &requests).await?;
        if raw.len() != chunk.len() {
            return Err(ctx.err(ErrorCode::BadResponse, "Неполный ответ со слотами Ekubo V2"));
        }
        for reply in raw {
            result.push(serde_json::from_str(reply?.get()).map_err(|_| {
                ctx.err(ErrorCode::BadResponse, "Неверное слово хранилища Ekubo V2")
            })?);
        }
    }
    Ok(result)
}

pub async fn load(
    ctx: &Ctx,
    f: &Family,
    core: Address,
    id: B256,
    block: u64,
) -> GatewayResult<crate::core::types::PoolState> {
    use super::{ekubo_keys, load_ekubo};
    use crate::core::types::{EkuboData, EkuboTick, PoolState};
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "Ekubo V2: ключ пула, реестр или состояние не подтверждены",
        )
    };
    let key = ekubo_keys::get(ctx.place.id, id).ok_or_else(bad)?;
    let config = math_config(key.config).ok_or_else(bad)?;
    let h = super::super::quote::white_state::head(ctx, &format!("{block:#x}")).await?;
    let tag = h.selector();
    let (state, _) = tokio::try_join!(
        rpc::call::<B256>(ctx, "eth_getStorageAt", json!([core, state_slot(id), tag])),
        check(ctx, f, core, &tag)
    )?;
    let (sqrt_ratio, tick, liquidity) = decode_state(&state);
    if sqrt_ratio.is_zero() {
        return Err(bad());
    }
    let spacing = u32::from_be_bytes(key.config[28..].try_into().map_err(|_| bad())?);
    let mut words = (0, 0);
    let mut bitmap = Vec::new();
    let mut ticks = Vec::new();
    if spacing != 0 {
        words =
            load_ekubo::words_around(tick, spacing, ctx.dex_int("dex.ekubo_bitmap_words", f.id)?);
        let keys: Vec<_> = (words.0..=words.1).map(|w| bitmap_slot(id, w)).collect();
        let bits = slots(ctx, core, &keys, &tag).await?;
        bitmap = (words.0..=words.1)
            .zip(bits)
            .filter(|(_, b)| !b.is_zero())
            .map(|(w, b)| (w, U256::from_be_bytes(b.0)))
            .collect();
        let at = load_ekubo::set_ticks(&bitmap, spacing);
        let keys: Vec<_> = at.iter().map(|&t| tick_slot(id, t)).collect();
        let values = slots(ctx, core, &keys, &tag).await?;
        for (t, value) in at.into_iter().zip(values) {
            let (net, delta) = super::abi_ekubo::decode_tick(&value);
            ticks.push(EkuboTick {
                tick: t,
                net,
                delta,
            });
        }
    }
    let native = if key.native {
        Some(ctx.addr("dex.native_wrapped")?)
    } else {
        None
    };
    Ok(PoolState::Ekubo(Box::new(EkuboData {
        config,
        sqrt_ratio,
        tick,
        liquidity,
        words,
        bitmap,
        ticks,
        mev: None,
        twamm: None,
        native,
        block_ts: h.timestamp.to::<u64>(),
    })))
}

pub fn order(e: &super::registry::Entry, input: Address, amount: U256) -> Option<OrderKey> {
    let crate::core::types::PoolState::Ekubo(d) = &e.pool.state else {
        return None;
    };
    let (token0, token1) = super::ekubo_native::key_tokens(d, e.pool.token0, e.pool.token1);
    let config = raw_config(d.config);
    let crate::core::types::PoolRef::Singleton { id, .. } = e.pool.reference else {
        return None;
    };
    if super::abi_ekubo::pool_id(token0, token1, config) != id
        || (input != e.pool.token0 && input != e.pool.token1)
    {
        return None;
    }
    let side = super::ekubo_native::is_token1(d, e.pool.token0, input);
    let amount = i128::try_from(amount).ok().filter(|n| *n > 0)?;
    let price_limit = price_limit(config, d.words, side)?;
    Some(OrderKey {
        price_limit,
        token0,
        token1,
        config,
        side,
        amount,
    })
}
