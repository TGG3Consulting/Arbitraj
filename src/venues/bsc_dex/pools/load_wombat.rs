//! Файл: venues/bsc_dex/pools/load_wombat.rs
//! Что делает: решение 484 — чтение состояния пула Wombat Exchange на блоке. Пул — прокси EIP-1967: реализация (слот
//! прокси на том же блоке) должна быть в dex.wombat_implementations (HighCovRatioFeePoolV2) или
//! dex.wombat_dynamic_implementations (DynamicPoolV2 — множитель _quoteFactor по getRelativePrice активов); иначе пул не
//! читается (код не сверен с исходником вида — котировки нет). Полное чтение (load_wombat_calls.rs): 1 — параметры пула
//! и реализация, 2 — адреса активов и паузы активов (слоты _pausedAssets пачкой JSON-RPC), 3 — cash / liability /
//! decimals / цена активов; монеты, активы и decimals запоминаются (load_wombat_memo.rs) до AssetAdded / AssetRemoved
//! или проверки пула — каждым блоком тогда одна пачка меняющихся величин и слоты. Всё — на одном блоке (тег).
//! Кто вызывает: pools/load.rs (ссылка Contract), pools/load_site.rs (пары многомонетного пула, решение 295 — одно
//! чтение на контракт), pools/verify_wombat.rs (kind_of).
//! От чего зависит: ctx.rs, node/call.rs (try_many), node/rpc.rs (call, batch), abi_wombat.rs, load_wombat_calls.rs,
//! load_wombat_memo.rs.
//! Настройки: dex.wombat_implementations, dex.wombat_dynamic_implementations [DEX вида wombat].
//!
//! Методы узла — execution-apis 16caee27 src/eth/state.yaml (eth_getStorageAt), src/eth/execute.yaml (eth_call).

use std::str::FromStr;

use alloy_primitives::{Address, B256, Bytes};
use serde_json::{Value, json};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::{call, rpc};
use super::abi_wombat::{implementation_slot, paused_slot};
use super::families::Family;
use super::load_wombat_calls::{self as c, Head, Side};
use super::load_wombat_memo as memo;

/// Настройка: реализации HighCovRatioFeePoolV2, сверенные с исходником.
pub const PLAIN: &str = "dex.wombat_implementations";
/// Настройка: реализации DynamicPoolV2, сверенные с исходником (необязательная).
pub const DYNAMIC: &str = "dex.wombat_dynamic_implementations";

/// Прочитанный пул (одно чтение на все пары пула — решение 295).
#[derive(Debug, Clone)]
pub struct Raw {
    pub pool: Address,
    pub head: Head,
    pub sides: Vec<Side>,
    pub asset_paused: Vec<bool>,
}

/// Адреса списка настройки. Принимает: окружение, вид, имя. Возвращает: адреса (не задана — пусто). Ошибки (текст):
/// настройка хранится не списком (Internal dex_list) — причина, элемент не адрес.
fn list(ctx: &Ctx, f: &Family, name: &str) -> Result<Vec<Address>, String> {
    let l = match ctx.dex_list(name, f.id) {
        Ok(l) => l,
        Err(e) if e.code == ErrorCode::NotConfigured => return Ok(Vec::new()),
        Err(e) => return Err(format!("{name}: {}", e.message)),
    };
    l.iter()
        .map(|s| Address::from_str(s.trim()).map_err(|_| format!("{name}: «{s}» — не адрес")))
        .collect()
}

/// Вид реализации пула. Принимает: окружение, вид DEX, адрес реализации. Возвращает: true — DynamicPoolV2, false —
/// HighCovRatioFeePoolV2; Err — реализации нет ни в одном списке (код не сверен) или список не разобрать.
pub fn kind_of(ctx: &Ctx, f: &Family, imp: Address) -> Result<bool, String> {
    if list(ctx, f, DYNAMIC)?.contains(&imp) {
        return Ok(true);
    }
    if list(ctx, f, PLAIN)?.contains(&imp) {
        return Ok(false);
    }
    Err(format!(
        "реализация пула {imp:#x} не в {PLAIN} / {DYNAMIC} (код не сверен с исходником вида wombat)"
    ))
}

/// Слова хранилища пачкой JSON-RPC. Принимает: окружение, (контракт, слот), тег. Возвращает: слова по порядку.
/// Ошибки: узел, ответ не слово.
async fn words(ctx: &Ctx, slots: &[(Address, B256)], tag: &str) -> GatewayResult<Vec<B256>> {
    let calls: Vec<(&str, Value)> = slots
        .iter()
        .map(|(a, s)| ("eth_getStorageAt", json!([a, s, tag])))
        .collect();
    let mut out = Vec::with_capacity(calls.len());
    for r in rpc::batch(ctx, &calls).await? {
        let raw = r?;
        let w: B256 = serde_json::from_str(raw.get()).map_err(|e| {
            ctx.err(ErrorCode::BadResponse, format!("eth_getStorageAt: не слово: {e}"))
        })?;
        out.push(w);
    }
    Ok(out)
}

/// Чтение пула на блоке. Монеты, активы и decimals (неизменные до AssetAdded / AssetRemoved) — из памяти
/// load_wombat_memo.rs: есть — одна пачка меняющихся величин (vary_calls) и слоты (реализация, паузы активов) пачкой
/// JSON-RPC одновременно; нет — полное чтение (full) и запись в память. Принимает: окружение, вид DEX, пул, тег блока.
/// Возвращает: Raw. Ошибки: узел, ответ не по ABI или откат геттера (BadResponse), реализация не сверена
/// (InvalidAddress).
pub async fn read(ctx: &Ctx, f: &Family, pool: Address, tag: &str) -> GatewayResult<Raw> {
    let Some(fx) = memo::get(f.id, pool) else {
        return full(ctx, f, pool, tag).await;
    };
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, format!("Wombat {pool:#x}: {w}"));
    let mut slots: Vec<(Address, B256)> = vec![(pool, implementation_slot())];
    slots.extend(fx.tokens.iter().map(|t| (pool, paused_slot(*t))));
    // Вид пула — по реализации того же блока; getRelativePrice читается всегда (у не динамического актива — откат,
    // ответ не используется), чтобы обойтись одной пачкой.
    let calls = c::vary_calls(pool, &fx.assets, true);
    let (got, read) = tokio::join!(call::try_many(ctx, &calls, tag), words(ctx, &slots, tag));
    let read = read?;
    let imp = Address::from_word(*read.first().ok_or_else(|| bad("нет слота реализации".into()))?);
    let dynamic = implementation(ctx, f, pool, imp)?;
    let got = got?;
    let got = if dynamic { got } else { without_price(got, fx.assets.len()) };
    let (head, sides) = c::parse_vary(&got, &fx, dynamic).map_err(bad)?;
    Ok(Raw {
        pool,
        head,
        sides,
        asset_paused: read[1..].iter().map(|w| !w.is_zero()).collect(),
    })
}

/// Ответы vary_calls(…, true) без ответов getRelativePrice (третий вызов каждого актива); тест на записи — тоже.
/// Принимает: ответы, число активов. Ошибок нет.
pub fn without_price(got: Vec<Result<Bytes, String>>, n: usize) -> Vec<Result<Bytes, String>> {
    got.into_iter()
        .enumerate()
        .filter(|(k, _)| *k < 5 || (*k - 5) % 3 != 2 || *k >= 5 + 3 * n)
        .map(|(_, r)| r)
        .collect()
}

/// Вид реализации или ошибка InvalidAddress с причиной. Ошибок иных нет.
fn implementation(ctx: &Ctx, f: &Family, pool: Address, imp: Address) -> GatewayResult<bool> {
    kind_of(ctx, f, imp)
        .map_err(|w| ctx.err(ErrorCode::InvalidAddress, format!("Wombat {pool:#x}: {w}")))
}

/// Полное чтение (три ступени со сверкой активов) и запись неизменных частей в память. Принимает и возвращает — как
/// read. Ошибки: как read.
async fn full(ctx: &Ctx, f: &Family, pool: Address, tag: &str) -> GatewayResult<Raw> {
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, format!("Wombat {pool:#x}: {w}"));
    let (calls, imp_slot) = (c::head_calls(pool), [(pool, implementation_slot())]);
    let (got, imp) = tokio::join!(call::try_many(ctx, &calls, tag), words(ctx, &imp_slot, tag));
    let head = c::parse_head(&got?).map_err(bad)?;
    let imp = Address::from_word(*imp?.first().ok_or_else(|| bad("нет слота реализации".into()))?);
    let dynamic = implementation(ctx, f, pool, imp)?;
    let slots: Vec<(Address, B256)> = head.tokens.iter().map(|t| (pool, paused_slot(*t))).collect();
    let calls = c::asset_calls(pool, &head.tokens);
    let (got, paused) = tokio::join!(call::try_many(ctx, &calls, tag), words(ctx, &slots, tag));
    let assets = c::parse_assets(&got?, &head.tokens).map_err(bad)?;
    let asset_paused = paused?.iter().map(|w| !w.is_zero()).collect();
    let got = call::try_many(ctx, &c::detail_calls(&assets, dynamic), tag).await?;
    let sides = c::parse_detail(&got, pool, &head.tokens, dynamic).map_err(bad)?;
    let fixed = memo::Fixed {
        tokens: head.tokens.clone(),
        assets,
        decimals: sides.iter().map(|s| s.decimals).collect(),
    };
    memo::put(f.id, pool, fixed);
    Ok(Raw {
        pool,
        head,
        sides,
        asset_paused,
    })
}

/// Состояние пары из прочитанного (без сети). Принимает: окружение (текст ошибки), Raw, пару. Ошибки: BadResponse.
pub fn state(ctx: &Ctx, raw: &Raw, pair: (Address, Address)) -> GatewayResult<PoolState> {
    c::build(&raw.head, &raw.sides, &raw.asset_paused, pair).map_err(|w| {
        let pool = raw.pool;
        ctx.err(ErrorCode::BadResponse, format!("Wombat {pool:#x}: {w}"))
    })
}

/// Чтение пары одним вызовом (ссылка Contract). Принимает: окружение, вид, пул, пару, тег. Ошибки: как read / state.
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let raw = read(ctx, f, pool, tag).await?;
    state(ctx, &raw, pair)
}
