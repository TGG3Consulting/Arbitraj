//! Файл: venues/bsc_dex/quote/depth_band.rs
//! Что делает: глубина пула ПО КАЖДОМУ НАПРАВЛЕНИЮ в полосе цены dex.depth_band_pct (решение 73), в USDT:
//! - продажа (монета → встречный токен): сколько USDT пул отдаст, пока цена монеты не упадёт на полосу;
//! - покупка (встречный токен → монета): сколько USDT уйдёт в пул, пока цена монеты не вырастет на полосу.
//!
//! Та же математика, что котировка: V2 — формула пары (резерв до цены на границе полосы), V3 / Infinity
//! CL / Uniswap V4 — обмен до предела цены (math/swap_cl.rs swap_to; граница полосы — тик ± Δ, Δ — сколько тиков
//! даёт полоса: цена тика 1,0001^tick — TickMath), Bin — до края прочитанных бинов (полосы у бинов пока нет — их
//! глубину ограничивает dex.bin_depth_usd при чтении). Полоса не прочитана целиком (кончились слова битмапа) — глубина
//! до края прочитанного и отметка `full = false`.
//!
//! Глубина для порога — БЕЗ комиссии пула в обе стороны (решение Тиграна к решению 73): чистый объём пула в полосе.
//! Комиссия — в котировке и прибыли (quote/pool_quote.rs), их этот файл не трогает.
//! Кто вызывает: pools/depth_book.rs.
//! От чего зависит: math/swap_cl.rs, math/tick_math.rs, math/v2.rs, quote/pool_quote.rs, quote/rate.rs, num.rs.
//! Настройки: полосу dex.depth_band_pct [bsc_dex] даёт вызывающий.
//!
//! Цена тика: sqrtPriceX96 = sqrt(1,0001^tick)·2^96 — Uniswap v3-core contracts/libraries/TickMath.sol (getSqrtRatioAtTick).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::types::{PoolKind, PoolState};

use super::super::ctx::Ctx;
use super::super::math::swap_cl::{self, ClState, Version};
use super::super::math::tick_math::{MAX_TICK, MIN_TICK, sqrt_ratio_at_tick};
use super::super::math::{bin_swap, v2};
use super::super::num;
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;
use super::rate;

/// Основание цены тика: цена тика = 1,0001^tick (TickMath) — свойство протокола, не настройка.
const TICK_BASE: f64 = 1.0001;
/// Глубина для порога — без комиссии пула (решение Тиграна к решению 73): чистый объём пула в полосе; комиссия
/// учитывается в котировке и прибыли (quote/pool_quote.rs), не здесь. 0 пипсов — это «без комиссии», не настройка.
const NO_FEE: u32 = 0;
/// Масштаб множителя резерва V2 (18 знаков) — точность расчёта, не порог.
const SCALE: f64 = 1e18;

/// Глубина по направлениям, USDT.
#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    /// Покупка монеты: USDT в пул до цены +полоса.
    pub buy: Decimal,
    /// Продажа монеты: USDT из пула до цены −полоса.
    pub sell: Decimal,
    /// Полоса прочитана целиком в обе стороны.
    pub full: bool,
}

/// Сколько тиков даёт полоса `b` (доля): вверх — ln(1+b)/ln(1,0001), вниз — −ln(1−b)/ln(1,0001), вниз до целого.
/// Ошибок нет: b вне (0, 1) — None.
pub fn band_ticks(b: f64, up: bool) -> Option<i32> {
    if !(b > 0.0 && b < 1.0) {
        return None;
    }
    let r = if up { (1.0 + b).ln() } else { -(1.0 - b).ln() };
    Some((r / TICK_BASE.ln()).floor() as i32)
}

/// Глубина пула по направлениям в полосе. Принимает: окружение, реестр, токены, запись, знаки USDT, полосу
/// dex.depth_band_pct (%). Ошибки (текст): полоса вне 0–100%, расчёт невозможен, нет курса встречного токена к USDT.
pub fn band_depth(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    e: &Entry,
    usdt_dec: u8,
    pct: Decimal,
) -> Result<Band, String> {
    let (buy, sell, full) = band_units(e, pct)?;
    let usdt = tokens.usdt(ctx).map_err(|e| e.message)?;
    let to = |a: U256| -> Result<Decimal, String> {
        let u = rate::to_usdt(ctx, reg, usdt, e.usdt, a).map_err(|e| e.message)?;
        num::to_decimal(u, usdt_dec).ok_or_else(|| "глубина не переводится в USDT".to_string())
    };
    Ok(Band {
        buy: to(buy)?,
        sell: to(sell)?,
        full,
    })
}

/// Глубина по направлениям во встречном токене пула (минимальные единицы): (покупка — отдано в пул, продажа —
/// получено из пула, полоса прочитана целиком). Принимает: запись, полосу %. Ошибки (текст): полоса вне
/// 0–100%, расчёт невозможен.
pub fn band_units(e: &Entry, pct: Decimal) -> Result<(U256, U256, bool), String> {
    let b = (pct / Decimal::ONE_HUNDRED)
        .to_f64()
        .filter(|b| *b > 0.0 && *b < 1.0);
    let b = b.ok_or_else(|| format!("dex.depth_band_pct = {pct}: нужно больше 0 и меньше 100"))?;
    let coin0 = e.coin == e.pool.token0;
    let (sell, s_full) = side(e, e.coin, coin0, b, false)?;
    let (buy, b_full) = side(e, e.usdt, !coin0, b, true)?;
    Ok((buy, sell, s_full && b_full))
}

/// Одно направление: сколько встречного токена (продажа — получено, покупка — отдано) до границы полосы.
/// Возвращает: (сумма во встречном токене, полоса прочитана целиком). Ошибки (текст): расчёт невозможен.
fn side(
    e: &Entry,
    token_in: Address,
    zfo: bool,
    b: f64,
    buy: bool,
) -> Result<(U256, bool), String> {
    let pick = |amount_in: U256, amount_out: U256| if buy { amount_in } else { amount_out };
    match (&e.pool.state, e.pool.kind) {
        (PoolState::V2 { reserve0, reserve1 }, PoolKind::V2) => {
            let (r_in, r_out) = if zfo {
                (*reserve0, *reserve1)
            } else {
                (*reserve1, *reserve0)
            };
            // Цена монеты сдвигается на полосу: продажа — резерв монеты растёт в 1/√(1−b), покупка — резерв встречного
            // в √(1+b) (x·y = k); без комиссии пары (решение Тиграна: глубина — чистый объём пула).
            let grow = if buy {
                (1.0 + b).sqrt() - 1.0
            } else {
                1.0 / (1.0 - b).sqrt() - 1.0
            };
            let f = U256::from((grow * SCALE) as u128);
            let eff =
                r_in.checked_mul(f).ok_or("переполнение резерва")? / U256::from(SCALE as u128);
            let none = (U256::from(1u8), U256::from(1u8));
            let out = v2::amount_out(eff, r_in, r_out, none).ok_or("расчёт пары V2 невозможен")?;
            Ok((pick(eff, out), true))
        }
        (
            PoolState::V3 {
                sqrt_price_x96,
                liquidity,
                tick,
            },
            PoolKind::V3,
        ) => {
            let ticks = e.ticks.as_ref().ok_or("карта тиков не прочитана")?;
            let s = ClState {
                sqrt_price: *sqrt_price_x96,
                tick: *tick,
                liquidity: *liquidity,
                fee: NO_FEE,
                ticks,
                version: Version::V3,
            };
            cl(&s, zfo, b, buy).map(|(i, o, f)| (pick(i, o), f))
        }
        (
            PoolState::Cl {
                sqrt_price_x96,
                tick,
                liquidity,
                ..
            },
            PoolKind::Cl,
        ) => {
            let ticks = e.ticks.as_ref().ok_or("карта тиков не прочитана")?;
            let s = ClState {
                sqrt_price: *sqrt_price_x96,
                tick: *tick,
                liquidity: *liquidity,
                fee: NO_FEE,
                ticks,
                version: Version::V4,
            };
            cl(&s, zfo, b, buy).map(|(i, o, f)| (pick(i, o), f))
        }
        // Bin: до края прочитанных бинов (полосы нет — см. шапку), без комиссии.
        (PoolState::Bin { active_id, .. }, PoolKind::Bin) => {
            let bins = e.bins.as_ref().ok_or("бины не прочитаны")?;
            let zfo_in = token_in == e.pool.token0;
            let r = bin_swap::swap(bins, *active_id, NO_FEE, zfo_in, true, u128::MAX)
                .ok_or("расчёт бинов невозможен")?;
            Ok((pick(r.amount_in, r.amount_out), false))
        }
        _ => Err("вид пула и состояние не совпадают".into()),
    }
}

/// CL: обмен до цены на границе полосы. Возвращает: (отдано, получено, дошли до границы). Ошибки (текст): расчёт.
fn cl(s: &ClState, zfo: bool, b: f64, buy: bool) -> Result<(U256, U256, bool), String> {
    // Граница — от текущего тика (не от точной цены внутри тика: расхождение меньше одного тика, 0,01%). Полоса уже
    // одного тика — глубины в ней нет.
    let d = band_ticks(b, buy).ok_or("полоса вне 0–100%")?;
    if d == 0 {
        return Ok((U256::ZERO, U256::ZERO, true));
    }
    let edge = if zfo { s.tick - d } else { s.tick + d }.clamp(MIN_TICK + 1, MAX_TICK - 1);
    let limit = sqrt_ratio_at_tick(edge).ok_or("цена границы полосы вне диапазона TickMath")?;
    let (r, reached) =
        swap_cl::swap_to(s, zfo, true, U256::from(u128::MAX), limit).map_err(str::to_string)?;
    Ok((r.amount_in, r.amount_out, reached))
}
