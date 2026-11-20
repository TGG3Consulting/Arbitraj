//! Файл: venues/bsc_dex/pools/load_crypto_tweak_v1.rs
//! Что делает: решение 472 (T472-03) — чтение состояния `tweak_price` и `_claim_admin_fees` пула crypto v1 фабрики
//! Curve (CurveCryptoSwap2ETH, 2 монеты) для модели откатов math/crypto_tweak_v1.rs:
//! - постоянные пула (token(), factory(), coins(0..1) — задаются один раз в initialize L1366-L1391, сеттеров нет) —
//!   в памяти по (площадка, пул); нет в памяти — одно чтение пачкой на том же блоке перед основным;
//! - в той же пачке, что состояние пула (load_crypto_v1.rs, после его вызовов): price_oracle(), xcp_profit(),
//!   xcp_profit_a(), virtual_price(), allowed_extra_profit(), adjustment_step(), admin_fee(), totalSupply() LP-токена,
//!   fee_receiver() фабрики, balanceOf(пул) обеих монет;
//! - параллельно — пачка JSON-RPC: eth_getStorageAt(пул, слот not_adjusted) и eth_getBalance(пул) (ETH монеты WETH20 —
//!   gulp L484-L485); их ответы дописываются в конец ответов пачки как слова по 32 байта.
//!
//! Слот not_adjusted = 33 (геттера нет, L153): раскладка Vyper 0.3.1 (`# @version 0.3.1`, L1) —
//! vyper v0.3.1 vyper/semantics/validation/data_positions.py set_storage_slots: сначала ключи nonreentrant (у пула
//! один — 'lock', L858…L1204 → слот 0), затем переменные модуля по порядку объявления, без constant (удаляются до
//! раскладки — vyper/ast/expansion.py L105-L107) и immutable (WETH20 L109 — в коде): token L112 → 1, coins L113 →
//! 2–3, price_scale L115 → 4, _price_oracle 5, last_prices 6, last_prices_timestamp 7, initial_A_gamma 8,
//! future_A_gamma 9, initial_A_gamma_time 10, future_A_gamma_time 11, allowed_extra_profit 12, future_… 13,
//! fee_gamma 14, future_fee_gamma 15, adjustment_step 16, future_… 17, ma_half_time 18, future_… 19, mid_fee 20,
//! out_fee 21, admin_fee 22, future_mid_fee 23, future_out_fee 24, future_admin_fee 25, balances L145 → 26–27, D 28,
//! factory L148 → 29, xcp_profit 30, xcp_profit_a 31, virtual_price 32, not_adjusted L153 → 33,
//! admin_actions_deadline 34. Сверка на записи (tools/arbitrum_record/curve_crypto.py): слоты 28, 31, 32, 34 = геттеры
//! D(), xcp_profit_a(), virtual_price(), admin_actions_deadline() (tests/arb_curve_crypto_tweak_v1_live.rs).
//! Монета WETH20 (immutable без геттера) — индекс get_eth_index(пул) фабрики (Factory.vy L422-L431; в постоянных);
//! у пула с такой монетой нужна dex.native_wrapped = эта монета (load_crypto_v1.rs state: не задана — NotConfigured,
//! пул ждёт настройку), у пула без ETH настройка не нужна. Прямой перевод токена на пул без события (gulp —
//! balanceOf) перечитывает пул механизм pools/transfer_in.rs (watched: CurveCryptoV1, CurveTricryptoV1), ETH без
//! события — per_block_crypto.rs (native_in).
//! Кто вызывает: pools/load_crypto_v1.rs (read, state).
//! От чего зависит: ctx.rs, node/call.rs, node/rpc.rs, abi_crypto_v1.rs, node/abi.rs (IERC20), gateway/shard_map.rs,
//! core/types (CryptoTweak, CryptoTweakV1).
//! Настройки: dex.native_wrapped [по площадке DEX] (монета WETH20); dex.multicall3 — через node/call.rs.
//!
//! Источник: curvefi/curve-factory-crypto 7ed1011 contracts/CurveCryptoSwap2ETH.vy (строки выше), Factory.vy L108,
//! CurveTokenV5.vy L42; eth_getStorageAt / eth_getBalance — ethereum/execution-apis src/eth/state.yaml.

use std::sync::LazyLock;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::core::types::pool_crypto::{CryptoTweak, CryptoTweakV1};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::shard_map::ShardMap;

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::{call, rpc};
use super::abi_crypto::ICryptoPool as P;
use super::abi_crypto_v1::ICryptoTweakV1 as T;

/// Слот not_adjusted в раскладке Vyper 0.3.1 (см. шапку).
pub const NOT_ADJUSTED_SLOT: u64 = 33;
/// Вызовов пачки tweak (без двух слов JSON-RPC в конце).
pub const CALLS: usize = 11;
/// Слот not_adjusted подтверждён записью своего узла (slot_check) — до записи false: модель не применяется,
/// причина — в журнал (load_crypto_tweak_list.rs unconfirmed).
pub const CONFIRMED: bool = false;

/// Постоянные пула: LP-токен, фабрика, монеты.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Statics {
    pub token: Address,
    pub factory: Address,
    pub coins: [Address; 2],
    /// Индекс монеты WETH фабрики (get_eth_index < 2); None — монеты ETH нет.
    pub eth: Option<usize>,
}

/// Постоянные по (площадка DEX, пул).
static STATICS: LazyLock<ShardMap<(&'static str, Address), Statics>> =
    LazyLock::new(ShardMap::default);

/// Вызовы постоянных: token(), factory(), coins(0), coins(1). Ошибок нет.
pub fn pre_calls(pool: Address) -> Vec<(Address, Bytes)> {
    let coin = |i: u8| P::coinsCall { i: U256::from(i) }.abi_encode();
    let data: [Vec<u8>; 4] = [
        T::tokenCall {}.abi_encode(),
        T::factoryCall {}.abi_encode(),
        coin(0),
        coin(1),
    ];
    data.into_iter().map(|d| (pool, d.into())).collect()
}

/// Разбор постоянных (без сети). Ошибки (текст): откат, ответ не по ABI.
pub fn pre_decode(got: &[Result<Bytes, String>]) -> Result<Statics, String> {
    let a = |i: usize| -> Result<Address, String> {
        match got.get(i) {
            Some(Ok(b)) => T::tokenCall::abi_decode_returns(b).map_err(|e| e.to_string()),
            Some(Err(r)) => Err(format!("постоянная {i} откатилась: {r}")),
            None => Err(format!("нет ответа постоянной {i}")),
        }
    };
    Ok(Statics {
        token: a(0)?,
        factory: a(1)?,
        coins: [a(2)?, a(3)?],
        eth: None,
    })
}

/// Постоянные пула: из памяти или чтением на блоке `tag` (запоминаются). Ошибки: узел, откат, ответ не по ABI.
pub async fn statics(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<Statics> {
    if let Some(s) = STATICS.get(&(ctx.place.id, pool)) {
        return Ok(s);
    }
    let got = call::try_many(ctx, &pre_calls(pool), tag).await?;
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, format!("Crypto v1 {pool}: {w}"));
    let mut s = pre_decode(&got).map_err(&bad)?;
    // Индекс монеты WETH — у фабрики пула (второе чтение, один раз на пул).
    let data: Bytes = T::get_eth_indexCall { pool }.abi_encode().into();
    let got = call::try_many(ctx, &[(s.factory, data)], tag).await?;
    let k = match got.first() {
        Some(Ok(b)) => call::decode::<T::get_eth_indexCall>(b)?,
        Some(Err(r)) => return Err(bad(format!("get_eth_index фабрики откатился: {r}"))),
        None => return Err(bad("нет ответа get_eth_index".into())),
    };
    s.eth = (k < U256::from(2u8)).then(|| k.to::<usize>());
    STATICS.insert((ctx.place.id, pool), s);
    Ok(s)
}

/// Ревью 472 (п. 6): у пула с монетой WETH фабрики (индекс eth) модель читает ETH пула — нужна dex.native_wrapped,
/// равная этой монете. Принимает: окружение, пул, монеты, индекс. Ошибки: NotConfigured — настройка не задана (пул ждёт
/// её, как прочие); BadResponse — настройка не равна монете WETH пула. Без монеты WETH — Ok (настройка не нужна).
pub fn need_wrapped(
    ctx: &Ctx,
    pool: Address,
    coins: &[Address],
    eth: Option<usize>,
) -> GatewayResult<()> {
    let Some(k) = eth else {
        return Ok(());
    };
    let why = format!("Crypto v1 {pool}: монета {k} — WETH фабрики, модель читает ETH пула");
    let w = match ctx.addr("dex.native_wrapped") {
        Ok(w) => w,
        Err(_) => return Err(ctx.not_set("dex.native_wrapped", why)),
    };
    if coins.get(k) == Some(&w) {
        return Ok(());
    }
    let have = coins.get(k);
    let what = format!("Crypto v1 {pool}: dex.native_wrapped {w} ≠ WETH пула {have:?}");
    Err(ctx.err(ErrorCode::BadResponse, what))
}

/// Постоянные пула из памяти (без сети). None — ещё не читались в этом запуске. Ошибок нет.
pub fn remembered(venue: &'static str, pool: Address) -> Option<Statics> {
    STATICS.get(&(venue, pool))
}

/// Вызовы tweak по порядку (CALLS штук; см. шапку). Принимает: пул, его постоянные. Ошибок нет.
pub fn calls(pool: Address, s: &Statics) -> Vec<(Address, Bytes)> {
    let at_pool: [Bytes; 7] = [
        T::price_oracleCall {}.abi_encode().into(),
        T::xcp_profitCall {}.abi_encode().into(),
        T::xcp_profit_aCall {}.abi_encode().into(),
        T::virtual_priceCall {}.abi_encode().into(),
        T::allowed_extra_profitCall {}.abi_encode().into(),
        T::adjustment_stepCall {}.abi_encode().into(),
        T::admin_feeCall {}.abi_encode().into(),
    ];
    let mut out: Vec<(Address, Bytes)> = at_pool.into_iter().map(|d| (pool, d)).collect();
    out.push((s.token, T::totalSupplyCall {}.abi_encode().into()));
    out.push((s.factory, T::fee_receiverCall {}.abi_encode().into()));
    let of: Bytes = IERC20::balanceOfCall { owner: pool }.abi_encode().into();
    out.extend(s.coins.iter().map(|c| (*c, of.clone())));
    out
}

/// Параметры пачки JSON-RPC: eth_getStorageAt(пул, слот not_adjusted, блок), eth_getBalance(пул, блок). Ошибок нет.
pub fn rpc_params(pool: Address, tag: &str) -> Vec<(&'static str, Value)> {
    let slot = format!("{NOT_ADJUSTED_SLOT:#x}");
    vec![
        ("eth_getStorageAt", json!([pool, slot, tag])),
        ("eth_getBalance", json!([pool, tag])),
    ]
}

/// Слово слота и баланс пула (пачка JSON-RPC) как два ответа по 32 байта. Ошибки: узел (вся пачка); ответ одного
/// запроса не по формату — его Err в списке.
pub async fn words(
    ctx: &Ctx,
    pool: Address,
    tag: &str,
) -> GatewayResult<Vec<Result<Bytes, String>>> {
    let got = rpc::batch(ctx, &rpc_params(pool, tag)).await?;
    let mut out = Vec::with_capacity(2);
    for (k, r) in got.into_iter().enumerate() {
        out.push(match r {
            Err(e) => Err(e.message),
            Ok(v) if k == 0 => slot_word(&v),
            Ok(v) => balance_word(&v),
        });
    }
    Ok(out)
}

/// Ответ eth_getStorageAt — слово 32 байта. Ошибки (текст): не слово.
pub fn slot_word(v: &RawValue) -> Result<Bytes, String> {
    match serde_json::from_str::<B256>(v.get()) {
        Ok(w) => Ok(Bytes::copy_from_slice(w.as_slice())),
        Err(e) => Err(format!("eth_getStorageAt: {e}")),
    }
}

/// Ответ eth_getBalance (число 0x…) — слово 32 байта. Ошибки (текст): не число.
fn balance_word(v: &RawValue) -> Result<Bytes, String> {
    match serde_json::from_str::<U256>(v.get()) {
        Ok(b) => Ok(Bytes::from(b.to_be_bytes::<32>().to_vec())),
        Err(e) => Err(format!("eth_getBalance: {e}")),
    }
}

/// Разбор (без сети). Принимает: ответы вызовов tweak и затем два слова JSON-RPC (пусто — не читались), монеты пула,
/// индекс монеты WETH (Statics.eth). Возвращает: Ok(None) — не читались; Ok(Some) — состояние; Err — откат, ответ
/// короче 32 байт.
pub fn decode(
    got: &[Result<Bytes, String>],
    coins: &[Address],
    eth: Option<usize>,
) -> Result<Option<CryptoTweak>, String> {
    if got.is_empty() {
        return Ok(None);
    }
    let w = |i: usize| -> Result<U256, String> {
        match got.get(i) {
            Some(Ok(b)) if b.len() >= 32 => Ok(U256::from_be_slice(&b[..32])),
            Some(Ok(_)) => Err(format!("ответ tweak v1 {i} короче 32 байт")),
            Some(Err(r)) => Err(format!("вызов tweak v1 {i} откатился: {r}")),
            None => Err(format!("нет ответа tweak v1 {i}")),
        }
    };
    // Монета WETH20 — ETH пула (последнее слово), прочие — balanceOf (вызовы 9, 10).
    let mut gulp = Vec::with_capacity(coins.len());
    for k in 0..coins.len() {
        gulp.push(if eth == Some(k) { w(CALLS + 1)? } else { w(9 + k)? });
    }
    Ok(Some(CryptoTweak {
        price_oracle: vec![w(0)?],
        total_supply: w(7)?,
        xcp_profit: w(1)?,
        virtual_price: w(3)?,
        allowed_extra_profit: w(4)?,
        adjustment_step: w(5)?,
        v1: Some(CryptoTweakV1 {
            not_adjusted: !w(CALLS)?.is_zero(),
            xcp_profit_a: w(2)?,
            admin_fee: w(6)?,
            fee_receiver: !w(8)?.is_zero(),
            gulp,
            factory: true,
            last_prices: Vec::new(),
        }),
    }))
}
