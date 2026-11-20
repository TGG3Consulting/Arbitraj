//! Файл: venues/bsc_dex/pools/load_lb20_bins.rs
//! Что делает: решение 445 — резервы непустых бинов пула Liquidity Book v2.0 в обе стороны от активного: тот же обход и
//! то же правило глубины, что у v2.1 (load_lb_bins.rs band: сторона читается, пока стоимость прочитанных бинов в USDT не
//! достигнет dex.bin_depth_usd, у пары без USDT — до конца), но следующий непустой бин — findFirstNonEmptyBinId(id,
//! swapForY) пула v2.0. Отличие v2.0: бина в эту сторону нет — вызов ОТКАТЫВАЕТСЯ (TreeMath__ErrorDepthSearch или
//! арифметика BitMath на краю дерева), а не возвращает 0 / 0xFFFFFF; функция только читает дерево бинов, других
//! откатов у неё нет — откат вызова (ответ узла «вызов откатился») = сторона прочитана до конца. Сбой узла — ошибка.
//! Края полосы «до конца» — те же константы bins.rs (NONE_ABOVE / NONE_BELOW), что у v2.1: off_map_bins.rs и
//! lb_swap.rs работают с полосой одинаково.
//! Кто вызывает: pools/load_lb20.rs (band).
//! От чего зависит: ctx.rs, node/call.rs (try_many), abi_lb20.rs, bins.rs, load_bin.rs (Depth, usdt_value),
//! load_lb_bins.rs (bin — getBin того же селектора).
//! Настройки: dex.bin_depth_usd [площадка DEX].
//!
//! Источник: lfj-gg/joe-v2, тег v2.0.0 (07f10212) — src/LBPair.sol L257-L259 (findFirstNonEmptyBinId →
//! _tree.findFirstBin(_id, _swapForY)), L265-L267 (getBin); src/libraries/TreeMath.sol L21-L67 (findFirstBin: swapForY →
//! правая сторона дерева = меньшие id; не найдено в глубине 0 — откат L60); src/libraries/BitMath.sol L13-L19
//! (closestBit: _bit − 1 / _bit + 1 вне unchecked — на краю откат solc 0.8.10).

use std::collections::BTreeMap;

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::U24;
use alloy_primitives::{Address, Bytes};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
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
/// Возвращает: Some(id) или None — откат вызова (бинов в эту сторону нет, см. шапку). Ошибки: узел, ответ не по ABI.
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
    let got = call::try_many(ctx, &[(pool, Bytes::from(c.abi_encode()))], tag).await?;
    match got.into_iter().next() {
        Some(Ok(b)) => Ok(Some(call::decode::<P::findFirstNonEmptyBinIdCall>(&b)?.to())),
        // Откат: Multicall3 — «вызов откатился: 0x…» (node/multicall.rs), пачка JSON-RPC — текст узла «execution
        // reverted» (errors.rs from_rpc). Иная ошибка части пачки — сбой узла, не край.
        Some(Err(w)) if reverted(&w) => Ok(None),
        Some(Err(w)) => Err(ctx.err(
            ErrorCode::BadResponse,
            format!("LB v2.0 {pool}: findFirstNonEmptyBinId({id}): {w}"),
        )),
        None => Err(ctx.err(
            ErrorCode::BadResponse,
            format!("LB v2.0 {pool}: нет ответа findFirstNonEmptyBinId({id})"),
        )),
    }
}

/// Ответ части пачки — откат вызова (а не сбой узла). Принимает: текст ответа. Ошибок нет.
pub fn reverted(w: &str) -> bool {
    w.contains("откатился") || w.to_ascii_lowercase().contains("execution reverted")
}
