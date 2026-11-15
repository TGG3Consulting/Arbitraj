//! Файл: venues/bsc_dex/pools/load_lb_bins.rs
//! Что делает: резервы непустых бинов пула Liquidity Book V2.1 (решение 290) в обе стороны от активного — тот же обход,
//! что у Infinity Bin (load_bin.rs), но геттерами самого пула LBPair: getBin(id) и getNextNonEmptyBin(swapForY, id).
//! Сторона читается, пока стоимость прочитанных бинов в USDT не достигнет dex.bin_depth_usd (или бины не кончатся). У
//! пары без USDT стоимость в USDT не считается — сторона читается до конца (точно; ограничение — DECISIONS 290).
//! Кто вызывает: pools/load_lb.rs.
//! От чего зависит: ctx.rs, node/call.rs, abi_lb.rs, bins.rs, load_bin.rs (usdt_value), tokens.rs.
//! Настройки: dex.bin_depth_usd [bsc_dex].
//!
//! Источник: lfj-gg/joe-v2@v2.1.0 — src/LBPair.sol L181-L183 (getBin: резервы X, Y), L193-L195 и L842-L844
//! (getNextNonEmptyBin: swapForY = true → TreeMath.findFirstRight — строго ниже id, нет — type(uint24).max;
//! swapForY = false → findFirstLeft — строго выше, нет — 0; src/libraries/math/TreeMath.sol L102-L147, L157-L202).

use std::collections::BTreeMap;

use alloy_primitives::Address;
use alloy_primitives::aliases::U24;
use rust_decimal::Decimal;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_lb::ILBPair as P;
use super::bins::BinMap;
use super::load_bin::{Depth, usdt_value};

/// Конец поиска вниз (findFirstRight): type(uint24).max (TreeMath.sol L147).
const NONE_BELOW: u32 = 0xff_ffff;
/// Конец поиска вверх (findFirstLeft): 0 (TreeMath.sol L202).
const NONE_ABOVE: u32 = 0;

/// Полоса бинов вокруг активного. Принимает: окружение, пул, активный бин, шаг, оценку стоимости (None — у пары нет USDT:
/// стороны читаются до конца), тег блока. Возвращает: BinMap (X, Y — токены пула X и Y). Ошибки: NotConfigured
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
    let (x, y) = bin(ctx, pool, active, tag).await?;
    if x != 0 || y != 0 {
        reserves.insert(active, (x, y));
    }
    let mut range = (active, active);
    for down in [true, false] {
        let mut cur = active;
        let mut value = Decimal::ZERO;
        loop {
            let c = P::getNextNonEmptyBinCall {
                swapForY: down,
                id: U24::from(cur),
            };
            let next: u32 = call::view(ctx, pool, &c, tag).await?.to();
            if (down && next == NONE_BELOW) || (!down && next == NONE_ABOVE) {
                // Дальше бинов нет: вся сторона известна.
                if down {
                    range.0 = 0
                } else {
                    range.1 = NONE_BELOW
                }
                break;
            }
            let (x, y) = bin(ctx, pool, next, tag).await?;
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

/// Резервы одного бина (X, Y). Ошибки: узел, ответ не по ABI.
pub async fn bin(ctx: &Ctx, pool: Address, id: u32, tag: &str) -> GatewayResult<(u128, u128)> {
    let r = call::view(ctx, pool, &P::getBinCall { id: U24::from(id) }, tag).await?;
    Ok((r.binReserveX, r.binReserveY))
}
