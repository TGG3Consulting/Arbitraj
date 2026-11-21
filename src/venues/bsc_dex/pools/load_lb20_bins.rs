//! Файл: venues/bsc_dex/pools/load_lb20_bins.rs
//! Что делает: решение 445 — резервы непустых бинов пула Liquidity Book v2.0 в обе стороны от активного: тот же обход и
//! то же правило глубины, что у v2.1 (load_lb_bins.rs band: сторона читается, пока стоимость прочитанных бинов в USDT не
//! достигнет dex.bin_depth_usd, у пары без USDT — до конца), но следующий непустой бин — findFirstNonEmptyBinId(id,
//! swapForY) пула v2.0. Отличие v2.0: бина в эту сторону нет — вызов ОТКАТЫВАЕТСЯ (TreeMath__ErrorDepthSearch или
//! арифметика BitMath на краю дерева — Panic(0x11)), а не возвращает 0 / 0xFFFFFF. Ревью 445: краем стороны считается
//! только откат с этими данными (edge); иной откат, откат без данных и сбой узла — ошибка чтения.
//! Края полосы «до конца» — те же константы bins.rs (NONE_ABOVE / NONE_BELOW), что у v2.1: off_map_bins.rs и
//! lb_swap.rs работают с полосой одинаково.
//! Кто вызывает: pools/load_lb20.rs (band).
//! От чего зависит: ctx.rs, node/call.rs (try_many), node/multicall.rs, node/rpc.rs (eth_call), abi_lb20.rs, bins.rs, load_bin.rs (Depth, usdt_value),
//! load_lb_bins.rs (bin — getBin того же селектора).
//! Настройки: dex.bin_depth_usd [площадка DEX].
//!
//! Источник: lfj-gg/joe-v2, тег v2.0.0 (07f10212) — src/LBPair.sol L257-L259 (findFirstNonEmptyBinId →
//! _tree.findFirstBin(_id, _swapForY)), L265-L267 (getBin); src/libraries/TreeMath.sol L21-L67 (findFirstBin: swapForY →
//! правая сторона дерева = меньшие id; не найдено в глубине 0 — откат L60); src/libraries/BitMath.sol L14-L20
//! (closestBit: _bit − 1 / _bit + 1 вне unchecked — на краю откат solc 0.8.10).

use std::collections::BTreeMap;

use alloy::sol_types::{Panic, PanicKind, SolCall, SolError};
use alloy_primitives::aliases::U24;
use alloy_primitives::{Address, Bytes};
use rust_decimal::Decimal;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::{call, multicall, rpc};
use super::abi_lb20::ILBPairV20 as P;
use super::bins::{BinMap, NONE_ABOVE, NONE_BELOW};
use super::load_bin::{Depth, usdt_value};
use super::load_lb_bins;

/// Полоса бинов вокруг активного. Принимает: окружение, пул, активный бин, шаг, оценку стоимости (None — у пары нет
/// USDT: стороны читаются до конца), тег блока. Возвращает: BinMap (X, Y — токены пула X и Y). Ошибки: NotConfigured
/// (dex.bin_depth_usd — только при оценке), узел, ответ не по ABI.
pub async fn band(
    ctx: &Ctx,
    pool: Address,
    active: u32,
    step: u32,
    depth: Option<&Depth>,
    tag: &str,
) -> GatewayResult<BinMap> {
    let limit = match depth {
        Some(_) => Some(ctx.dec("dex.bin_depth_usd")?),
        None => None,
    };
    let mut reserves = BTreeMap::new();
    let (x, y) = load_lb_bins::bin(ctx, pool, active, tag).await?;
    if x != 0 || y != 0 {
        reserves.insert(active, (x, y));
    }
    let mut range = (active, active);
    for down in [true, false] {
        let mut cur = active;
        let mut value = Decimal::ZERO;
        loop {
            let Some(next) = next_bin(ctx, pool, cur, down, tag).await? else {
                // Дальше бинов нет: вся сторона известна (края — как у v2.1, bins.rs).
                if down {
                    range.0 = NONE_ABOVE
                } else {
                    range.1 = NONE_BELOW
                }
                break;
            };
            let (x, y) = load_lb_bins::bin(ctx, pool, next, tag).await?;
            reserves.insert(next, (x, y));
            if down {
                range.0 = next
            } else {
                range.1 = next
            }
            if let (Some(d), Some(l)) = (depth, limit) {
                value += usdt_value(ctx, x, y, next, step, d)?;
                if value >= l {
                    break;
                }
            }
            cur = next;
        }
    }
    Ok(BinMap {
        step,
        reserves: reserves.into(),
        range,
    })
}

/// Следующий непустой бин (down — вниз по id, swapForY = true). Принимает: окружение, пул, бин, сторону, тег блока.
/// Возвращает: Some(id) или None — откат с данными края (edge: бинов в эту сторону нет, см. шапку). Ошибки: узел,
/// ответ не по ABI, иной откат или откат без данных (BadResponse / ошибка узла).
pub async fn next_bin(
    ctx: &Ctx,
    pool: Address,
    id: u32,
    down: bool,
    tag: &str,
) -> GatewayResult<Option<u32>> {
    let c = P::findFirstNonEmptyBinIdCall {
        id_: U24::from(id),
        sentTokenY: down,
    };
    let data = Bytes::from(c.abi_encode());
    let not_edge = |w: &str| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("LB v2.0 {pool}: findFirstNonEmptyBinId({id}): откат не на краю: {w}"),
        )
    };
    // Multicall3: откат части пачки — текст «вызов откатился: 0x<данные>» (node/multicall.rs).
    if multicall::address(ctx).is_some() {
        let got = call::try_many(ctx, &[(pool, data)], tag).await?;
        return match got.into_iter().next() {
            Some(Ok(b)) => Ok(Some(
                call::decode::<P::findFirstNonEmptyBinIdCall>(&b)?.to(),
            )),
            Some(Err(w)) if mc_data(&w).is_some_and(|d| edge(&d)) => Ok(None),
            Some(Err(w)) => Err(not_edge(w.as_str())),
            None => Err(not_edge("нет ответа")),
        };
    }
    // Без Multicall3: eth_call, данные отката — поле error.data сырого ответа узла (errors.rs from_rpc with_raw).
    let req = json!([{ "to": pool, "input": data }, tag]);
    match rpc::call::<Bytes>(ctx, "eth_call", req).await {
        Ok(b) => Ok(Some(
            call::decode::<P::findFirstNonEmptyBinIdCall>(&b)?.to(),
        )),
        Err(e) if raw_edge(e.raw.as_deref()) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Данные отката «бинов в эту сторону нет» (см. шапку): ровно селектор TreeMath__ErrorDepthSearch() или
/// Panic(0x11) — переполнение uint8 в BitMath.closestBit на краю дерева (код паники — документация Solidity, «Panic via
/// assert and Error via require»). Принимает: данные отката. Ошибок нет.
pub fn edge(data: &[u8]) -> bool {
    data == &P::TreeMath__ErrorDepthSearch::SELECTOR[..]
        || data
            == Panic::from(PanicKind::UnderOverflow)
                .abi_encode()
                .as_slice()
}

/// Данные отката из текста части пачки Multicall3 («вызов откатился: 0x…»). None — не тот вид. Ошибок нет.
pub fn mc_data(w: &str) -> Option<Vec<u8>> {
    alloy_primitives::hex::decode(w.strip_prefix("вызов откатился: ")?).ok()
}

/// Сырой ответ узла — откат с данными края (rpc_data, edge). Ошибок нет.
fn raw_edge(raw: Option<&str>) -> bool {
    raw.and_then(rpc_data).is_some_and(|d| edge(&d))
}

/// Данные отката из сырого ответа JSON-RPC (error.data). None — поля нет или не hex. Ошибок нет.
pub fn rpc_data(raw: &str) -> Option<Vec<u8>> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    alloy_primitives::hex::decode(v["error"]["data"].as_str()?).ok()
}
