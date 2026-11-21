//! Файл: venues/bsc_dex/pools/load_crypto_tweak_list.rs
//! Что делает: решение 472 (T472-03) — чтение состояния `tweak_price` crypto-пулов Curve первого поколения без фабрики
//! (протокол tricrypto 2021 из списка dex.stable_pools): tricrypto 2021 (3 монеты, модель math/crypto_tweak_v1_3.rs)
//! и пара CurveCryptoSwap2 (EURS/2CRV, вариант CryptoPair, модель math/crypto_tweak_v1.rs с factory = false).
//! - постоянные пула (token(), coins(k) — константы исходника) — в памяти по (площадка, пул); нет — одно чтение на
//!   том же блоке;
//! - в пачке вызовов после прежних (load_crypto_v1.rs, после is_killed): пара — last_prices(), last_prices_timestamp(),
//!   ma_half_time(), price_oracle(), xcp_profit(), xcp_profit_a(), virtual_price(), allowed_extra_profit(),
//!   adjustment_step(), admin_fee(), admin_fee_receiver(), totalSupply() токена, balanceOf(пул) монет; tricrypto —
//!   last_prices(0..1), last_prices_timestamp(), ma_half_time(), price_oracle(0..1), xcp_profit(), virtual_price(),
//!   allowed_extra_profit(), adjustment_step(), totalSupply() токена;
//! - параллельно — eth_getStorageAt слота not_adjusted (последним словом).
//!
//! Оракул у обоих — сохранённое значение; tweak_price пересчитывает его на время блока обмена: здесь — на время блока
//! чтения (math/crypto_math3.rs oracle_at; tricrypto — ещё упаковка `assert p < PRICE_MASK` L460-L466); откат пересчёта
//! — оракул пуст (модель отказывает: обмен откатился бы).
//!
//! Слоты not_adjusted — раскладка по исходнику компилятора: в Vyper 0.2.15 и 0.3.0 КАЖДАЯ функция с @nonreentrant
//! получает свой слот (vyper v0.2.15 и v0.3.0 vyper/semantics/validation/data_positions.py set_storage_slots: цикл по
//! FunctionDef — `storage_slot += 1` на функцию; ключ один на имя — только с v0.3.1), затем переменные модуля по
//! порядку, без constant (expansion.py L102-L104):
//! - tricrypto («2021-09-13. Arbitrum/CryptoSwap.vy», `# @version 0.2.15`): 6 функций (L592, L727, L832, L931, L960,
//!   L1082) → 0–5; price_scale_packed L119 → 6 … future_admin_fee L147 → 27, balances L149 → 28–30, D 31, owner 32,
//!   future_owner 33, xcp_profit 34, xcp_profit_a 35, virtual_price 36, **not_adjusted L158 → 37**, is_killed 38,
//!   kill_deadline 39, transfer_ownership_deadline 40, admin_actions_deadline 41;
//! - EURS («2021-10-30. EURS/CryptoSwap.vy», `# @version 0.3.0`): 6 функций (L686, L816, L909, L1011, L1042, L1164)
//!   → 0–5; price_scale L97 → 6, price_oracle 7, last_prices 8, last_prices_timestamp 9 … future_admin_fee L125 → 27,
//!   balances L127 → 28–29, D 30, owner 31, future_owner 32, xcp_profit 33, xcp_profit_a 34, virtual_price 35,
//!   **not_adjusted L136 → 36**, is_killed 37, kill_deadline 38, transfer_ownership_deadline 39,
//!   admin_actions_deadline 40.
//!
//! Номер слота подтверждается записью (tools/arbitrum_record/curve_crypto.py, slot_check соседних слотов). Пока не
//! подтверждён (CONFIRMED = false) — вызовы tweak не читаются, модель не применяется (котировка как раньше), причина —
//! один раз в журнал по пулу (unconfirmed).
//! Кто вызывает: pools/load_crypto_v1.rs (read, state), pools/load_crypto_tweak_v1.rs (unconfirmed).
//! От чего зависит: ctx.rs, node/call.rs, node/rpc.rs, abi_crypto.rs, abi_crypto_v1.rs, node/abi.rs (IERC20),
//! load_crypto_tweak_v1.rs (words), math/crypto_math3.rs, math/crypto_tweak_v1_3.rs (price_mask),
//! gateway/shard_map.rs, storage/journal (Level).
//! Настройки: нет (dex.multicall3 — через node/call.rs).

use std::sync::LazyLock;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::shard_map::ShardMap;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::{call, rpc};
use super::abi_crypto::ICryptoPool as P;
use super::abi_crypto_v1::{ICryptoTweakPair as R, ICryptoTweakTri as T3, ICryptoTweakV1 as T};
use super::load_crypto_tweak_v1::slot_word;

/// Разбор ответов — load_crypto_tweak_list_decode.rs (вынесен; путь прежний).
pub use super::load_crypto_tweak_list_decode::decode;

/// Слот not_adjusted tricrypto 2021 (Vyper 0.2.15, см. шапку).
pub const TRI_SLOT: u64 = 37;
/// Слот not_adjusted пары CurveCryptoSwap2 (Vyper 0.3.0, см. шапку).
pub const PAIR_SLOT: u64 = 36;
/// Слоты подтверждены записью своего узла (slot_check) — до записи false: модель не применяется.
pub const CONFIRMED: bool = false;

/// Постоянные пула: токен LP и монеты.
pub type Statics = (Address, Vec<Address>);

/// Постоянные по (площадка DEX, пул).
static STATICS: LazyLock<ShardMap<(&'static str, Address), Statics>> =
    LazyLock::new(ShardMap::default);
/// Пулы, о которых причина «слот не подтверждён» уже записана в журнал.
static NOTED: LazyLock<ShardMap<(&'static str, Address), ()>> = LazyLock::new(ShardMap::default);

/// Слот not_adjusted по числу монет (3 — tricrypto, 2 — пара). Ошибок нет.
pub fn slot(n: usize) -> u64 {
    if n == 3 { TRI_SLOT } else { PAIR_SLOT }
}

/// Причина «модель tweak_price не применяется» — один раз на пул в журнал (отдельной задачей). Ошибок нет.
pub fn unconfirmed(ctx: &Ctx, pool: Address, what: &str) {
    if NOTED.insert((ctx.place.id, pool), ()).is_some() {
        return;
    }
    let msg = format!(
        "{pool}: модель отката tweak_price не применяется — {what}: номер слота not_adjusted не подтверждён записью \
         узла (tools/arbitrum_record/curve_crypto.py, решение 472 T472-03); котировка — без этой модели"
    );
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let ctx = ctx.clone();
        rt.spawn(async move { ctx.log(Level::Warn, "dex.pool", msg, None).await });
    }
}

/// Постоянные пула: из памяти или чтением token(), coins(0..n−1) на блоке `tag`. Ошибки: узел, откат, не по ABI.
pub async fn statics(ctx: &Ctx, pool: Address, n: usize, tag: &str) -> GatewayResult<Statics> {
    if let Some(s) = STATICS.get(&(ctx.place.id, pool)) {
        return Ok(s);
    }
    let mut calls: Vec<(Address, Bytes)> = vec![(pool, T::tokenCall {}.abi_encode().into())];
    for k in 0..n {
        calls.push((pool, P::coinsCall { i: U256::from(k) }.abi_encode().into()));
    }
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != n + 1 {
        return Err(bad(
            ctx,
            pool,
            format!("постоянных {} из {}", got.len(), n + 1),
        ));
    }
    let mut a = Vec::with_capacity(n + 1);
    for (k, r) in got.iter().enumerate() {
        match r {
            Ok(b) => a.push(call::decode::<T::tokenCall>(b)?),
            Err(w) => return Err(bad(ctx, pool, format!("постоянная {k} откатилась: {w}"))),
        }
    }
    let s = (a[0], a[1..].to_vec());
    STATICS.insert((ctx.place.id, pool), s.clone());
    Ok(s)
}

/// Слово слота not_adjusted на блоке `tag` (eth_getStorageAt) — один ответ. Ошибки: узел; не слово — Err в списке.
pub async fn words(
    ctx: &Ctx,
    pool: Address,
    n: usize,
    tag: &str,
) -> GatewayResult<Vec<Result<Bytes, String>>> {
    let at = format!("{:#x}", slot(n));
    let v = rpc::call_raw(ctx, "eth_getStorageAt", json!([pool, at, tag])).await?;
    Ok(vec![slot_word(&v)])
}

/// Ошибка чтения с пулом. Ошибок нет.
fn bad(ctx: &Ctx, pool: Address, w: String) -> GatewayError {
    ctx.err(ErrorCode::BadResponse, format!("Crypto v1 {pool}: {w}"))
}

/// Вызовы tweak по порядку (см. шапку). Принимает: пул, число монет, токен LP, монеты. Ошибок нет.
pub fn calls(pool: Address, n: usize, token: Address, coins: &[Address]) -> Vec<(Address, Bytes)> {
    let k = |i: u8| U256::from(i);
    let at_pool: Vec<Vec<u8>> = if n == 3 {
        vec![
            T3::last_pricesCall { k: k(0) }.abi_encode(),
            T3::last_pricesCall { k: k(1) }.abi_encode(),
            R::last_prices_timestampCall {}.abi_encode(),
            R::ma_half_timeCall {}.abi_encode(),
            T3::price_oracleCall { k: k(0) }.abi_encode(),
            T3::price_oracleCall { k: k(1) }.abi_encode(),
            T::xcp_profitCall {}.abi_encode(),
            T::virtual_priceCall {}.abi_encode(),
            T::allowed_extra_profitCall {}.abi_encode(),
            T::adjustment_stepCall {}.abi_encode(),
        ]
    } else {
        vec![
            R::last_pricesCall {}.abi_encode(),
            R::last_prices_timestampCall {}.abi_encode(),
            R::ma_half_timeCall {}.abi_encode(),
            T::price_oracleCall {}.abi_encode(),
            T::xcp_profitCall {}.abi_encode(),
            T::xcp_profit_aCall {}.abi_encode(),
            T::virtual_priceCall {}.abi_encode(),
            T::allowed_extra_profitCall {}.abi_encode(),
            T::adjustment_stepCall {}.abi_encode(),
            T::admin_feeCall {}.abi_encode(),
            R::admin_fee_receiverCall {}.abi_encode(),
        ]
    };
    let mut out: Vec<(Address, Bytes)> = at_pool.into_iter().map(|d| (pool, d.into())).collect();
    out.push((token, T::totalSupplyCall {}.abi_encode().into()));
    if n == 2 {
        let of: Bytes = IERC20::balanceOfCall { owner: pool }.abi_encode().into();
        out.extend(coins.iter().map(|c| (*c, of.clone())));
    }
    out
}
