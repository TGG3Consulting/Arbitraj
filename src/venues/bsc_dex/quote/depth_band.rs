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
//! Algebra (THENA Fusion, Algebra Integral) — quote/depth_band_algebra.rs (решение 205); пустая пара V2 / Solidly — глубина 0
//! (решение 205), а не ошибка.
//! Решение 307: band_from_rate — перевод в USDT по пулам, взятым один раз на подготовку (pools/depth_key.rs Rate).
//! Кто вызывает: pools/depth_book.rs.
//! От чего зависит: math/swap_cl.rs, math/tick_math.rs, math/v2.rs, math/solidly_v1_math.rs (решение 483: variant_out), quote/pool_quote.rs, quote/rate.rs, num.rs.
//! Настройки: полосу dex.depth_band_pct [bsc_dex] даёт вызывающий.
//!
//! Цена тика: sqrtPriceX96 = sqrt(1,0001^tick)·2^96 — Uniswap v3-core contracts/libraries/TickMath.sol (getSqrtRatioAtTick).

use std::sync::Arc;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::types::{PoolKind, PoolState};

use super::super::ctx::Ctx;
use super::super::math::swap_cl::{self, ClState, Version};
use super::super::math::swap_elastic::{self, ElasticState};
use super::super::math::tick_math::{MAX_TICK, MIN_TICK, sqrt_ratio_at_tick};
use super::super::math::{bin_swap, dmm, solidly_v1_math, v2};
use super::super::num;
use super::super::pools::family_variant::Variant;
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
    let units = band_units(e, pct)?;
    band_from_units(ctx, reg, tokens, e, usdt_dec, units)
}

/// Решение 301 (б): перевод глубины во встречном токене (band_units) в USDT — вторая половина band_depth, отдельно,
/// чтобы книга глубины (pools/depth_book.rs) запоминала неудачу только расчёта по состоянию пула. Принимает: окружение,
/// реестр, токены, запись, знаки USDT, итог band_units. Возвращает: глубину в USDT. Ошибки (текст): нет USDT, нет курса
/// встречного токена к USDT, глубина не переводится в USDT.
pub fn band_from_units(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    e: &Entry,
    usdt_dec: u8,
    (buy, sell, full): (U256, U256, bool),
) -> Result<Band, String> {
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

/// Решение 307: то же, что band_from_units, но пулы «встречный токен — USDT» уже взяты (depth_key.rs Prep::rate — один
/// раз на подготовку, вместе с их поколениями в ключе глубины), а не by_pair на каждый to_usdt. Перевод — тот же своп
/// всей суммы через лучший достоверный пул (rate::best_in; смысл решения 73 не меняется). Принимает: как
/// band_from_units, и пулы встречного токена (None — как band_from_units: встречный USDT или USDT не известен
/// подготовке; решение 410 (t195) — `Arc` записей реестра, без копий). Ошибки: как у band_from_units.
pub fn band_from_rate(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    e: &Entry,
    usdt_dec: u8,
    units: (U256, U256, bool),
    rate: Option<&[Arc<Entry>]>,
) -> Result<Band, String> {
    let Some(pools) = rate else {
        return band_from_units(ctx, reg, tokens, e, usdt_dec, units);
    };
    let usdt = tokens.usdt(ctx).map_err(|e| e.message)?;
    if e.usdt == usdt {
        return band_from_units(ctx, reg, tokens, e, usdt_dec, units);
    }
    let (buy, sell, full) = units;
    let to = |a: U256| -> Result<Decimal, String> {
        let u = rate::best_in(ctx, pools, e.usdt, e.usdt, a).map_err(|e| e.message)?;
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
        (PoolState::UmbraeDamm(s), PoolKind::V2) => {
            let i = s.tokens.iter().position(|&t| t == token_in).ok_or("Umbrae DAMM: неизвестный токен")?;
            let (budget, _) = xy_band(s.reserves[i], s.reserves[1-i], b, buy)?;
            if budget.is_zero() {return Ok((budget,true));}
            let (amount, full) = super::super::math::umbrae_damm::band(s, i, budget, buy)
                .ok_or("Umbrae DAMM: неверные границы резервов")?;
            Ok((amount,full))
        }
        (PoolState::Bundles(book),PoolKind::V2) => {
            let i=usize::from(book.market.tokens[1]==token_in);
            let grow=if buy{(1.0+b).sqrt()-1.0}else{1.0/(1.0-b).sqrt()-1.0};
            let budget=super::super::math::full_math::mul_div(book.market.reserves[i],U256::from((grow*SCALE) as u128),U256::from(SCALE as u128)).ok_or("Bundles Swap: граница превышает резерв")?;
            let amount=book.swap(token_in,true,budget).map_or(U256::ZERO,|(s,_)|pick(s.amount_in,s.amount_out));
            Ok((amount,false))
        }
        (PoolState::Antfarm(book),PoolKind::V2) => {
            let i=usize::from(book.market.tokens[1]==token_in);
            let grow=if buy{(1.0+b).sqrt()-1.0}else{1.0/(1.0-b).sqrt()-1.0};
            let budget=super::super::math::full_math::mul_div(book.market.reserves[i],U256::from((grow*SCALE) as u128),U256::from(SCALE as u128)).ok_or("Antfarm: граница превышает размер резерва")?;
            let amount=book.swap(token_in,true,budget).map_or(U256::ZERO,|p|pick(p.amount_in,p.amount_out));
            Ok((amount,false))
        }
        (PoolState::Temple(s),PoolKind::V2) => {
            let out=if zfo{e.pool.token1}else{e.pool.token0};
            let grow=if buy{(1.0+b).sqrt()-1.0}else{1.0/(1.0-b).sqrt()-1.0};
            Ok((super::super::math::temple::band(s,token_in,out,U256::from((grow*SCALE) as u128),U256::from(SCALE as u128),buy),true))
        }
        (PoolState::YieldBasisLev(s),PoolKind::V2)=>{
            let (x,y)=s.reserves(token_in).ok_or("YieldBasis: виртуальные резервы недоступны")?;
            let (budget,_)=xy_band(x,y,b,true)?;
            let qty=if buy{budget}else{super::super::math::full_math::mul_div(x,U256::from(((1.0/(1.0-b).sqrt()-1.0)*SCALE) as u128),U256::from(SCALE as u128)).ok_or("YieldBasis: переполнение полосы")?};
            Ok((s.swap(token_in,qty).map_or(U256::ZERO,|f|pick(f.spent,f.received)),false))
        }
        (PoolState::BalancerV1(s),PoolKind::V2)=>{
            let output=if zfo{e.pool.token1}else{e.pool.token0};
            let width=U256::from((b*SCALE) as u64);
            let scale=U256::from(SCALE as u64);
            let price=s.spot_price(token_in,output).ok_or("Balancer V1: цена пула недоступна")?;
            let limit=if buy{super::super::math::full_math::mul_div(price,scale+width,scale)}
                else{super::super::math::full_math::mul_div(price,scale,scale-width)}.ok_or("Balancer V1: переполнение полосы")?;
            let f=s.band(token_in,output,limit);
            Ok((f.map_or(U256::ZERO,|v|pick(v.spent,v.received)),false))
        }
        (PoolState::BancorV3(s),PoolKind::V2)=>{
            let Some((x,y))=s.band_reserves(token_in) else{return Ok((U256::ZERO,true));};
            let (input,_)=xy_band(x,y,b,true)?;
            let grow=if buy{input}else{
                let factor=U256::from(((1.0/(1.0-b).sqrt()-1.0)*SCALE) as u128);
                super::super::math::full_math::mul_div(x,factor,U256::from(SCALE as u128)).ok_or("Bancor V3: переполнение глубины")?
            };
            if let Some(f)=s.swap(token_in,true,grow,false){return Ok((pick(f.spent,f.received),true));}
            let cap=s.vault_balances[usize::from(token_in==s.tokens[0])];
            Ok((s.swap(token_in,false,cap,false).filter(|f|f.spent<=grow).map_or(U256::ZERO,|f|pick(f.spent,f.received)),false))
        }
        (PoolState::UniswapV1(s),PoolKind::V2)=>{
            let (x,y)=s.band_reserves(token_in).ok_or("Uniswap V1: нет физической пары")?;
            xy_band(x,y,b,buy)
        }
        (PoolState::Rubicon(book), PoolKind::V2) => book.depth_in_band(token_in, buy, (b*SCALE) as u64, SCALE as u64)
            .map(|value|(value,true)).ok_or_else(||"Rubicon Classic: некорректная глубина книги".into()),
        (PoolState::Carbon(s),PoolKind::V2) => super::super::math::carbon_depth::band(s,token_in,buy,(b*SCALE) as u64)
            .map(|n|(n,true)).ok_or_else(||"Carbon: не удалось рассчитать глубину заявок".into()),
        (PoolState::MintClub(s),PoolKind::V2) => super::super::math::mint_club_depth::band(s,token_in,buy,(b*SCALE) as u64)
            .ok_or_else(||"Mint Club: не вычислена исполнимая глубина ступеней".into()),
        (PoolState::Virtuals(s), PoolKind::V2) => {
            let i=usize::from(token_in==s.asset);let o=1-i;
            let grow=if buy{(1.0+b).sqrt()-1.0}else{1.0/(1.0-b).sqrt()-1.0};
            let input=s.reserves[i].checked_mul(U256::from((grow*SCALE) as u128)).ok_or("Virtuals: переполнение полосы")?/U256::from(SCALE as u128);
            if input.is_zero(){return Ok((U256::ZERO,true));}
            let remaining=s.invariant.checked_div(s.reserves[i].checked_add(input).ok_or("Virtuals: переполнение резерва")?).ok_or("Virtuals: пустой резерв")?;
            let output=s.reserves[o].saturating_sub(remaining);
            let mut cap=s.backing[o];
            if i==1 && let Some(threshold)=s.graduation {cap=cap.min(s.reserves[o].saturating_sub(threshold.saturating_add(U256::ONE)));}
            if output<=cap{return Ok((pick(input,output),true));}
            let floor=s.reserves[o]-cap;
            let capped=(s.invariant/floor).saturating_sub(s.reserves[i]).min(input);
            let left=s.invariant.checked_div(s.reserves[i].saturating_add(capped)).ok_or("Virtuals: пустой входной резерв")?;
            let received=s.reserves[o].saturating_sub(left);
            Ok((pick(capped,received),false))
        }
        (PoolState::Memebox(s), PoolKind::V2) => {
            let i=usize::from(!zfo);
            xy_band(s.reserves[i],s.reserves[1-i],b,buy)
        }
        (PoolState::Surge(s),PoolKind::V2) => {
            let native=token_in==s.wrapped;
            let rin=if native{s.liquidity}else{s.stock};
            let grow=if buy{(1.0+b).sqrt()-1.0}else{1.0/(1.0-b).sqrt()-1.0};
            let wanted=super::super::math::full_math::mul_div(rin,U256::from((grow*SCALE) as u128),U256::from(SCALE as u128)).ok_or("Surge: переполнение полосы")?;
            let cap=if native{s.capacity.min(s.stock)}else{s.cash};
            let sweep=s.swap(token_in,true,wanted).or_else(||s.swap(token_in,false,cap));
            Ok((sweep.filter(|x|x.amount_in<=wanted).map_or(U256::ZERO,|x|pick(x.amount_in,x.amount_out)),false))
        }
        (PoolState::Dagrow(s), PoolKind::V2) => {
            let side=usize::from(token_in==s.dagr);
            if s.fees[side].is_none() || s.stable_scale.is_zero() {return Ok((U256::ZERO,true));}
            let raw=[s.reserves[0]/s.stable_scale,s.reserves[1]];
            xy_band(raw[side],raw[1-side],b,buy)
        }
        (PoolState::ToonSwap(s), PoolKind::V2) => { let i=usize::from(!zfo); xy_band(s.reserves[i],s.reserves[1-i],b,buy) }
        (PoolState::CandySwap(s), PoolKind::V2) => {
            let i=usize::from(!zfo);
            xy_band(s.reserves[i],s.reserves[1-i],b,buy)
        }
        (PoolState::EtherVista(s), PoolKind::V2) => {
            let (rin,rout) = if zfo { (s.reserve0,s.reserve1) } else { (s.reserve1,s.reserve0) };
            xy_band(rin,rout,b,buy)
        }
        (
            PoolState::V2 { reserve0, reserve1 } | PoolState::Oasis { reserve0, reserve1, .. }
            | PoolState::DeltaSwap {
                reserve0, reserve1, ..
            },
            PoolKind::V2,
        ) => {
            let (r_in, r_out) = if zfo {
                (*reserve0, *reserve1)
            } else {
                (*reserve1, *reserve0)
            };
            xy_band(r_in,r_out,b,buy)
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
        // KyberSwap Elastic (решение 81): обмен до границы полосы своей математикой; без комиссии пула (fee_units = 0).
        (
            PoolState::Elastic {
                sqrt_price_x96,
                tick,
                base_l,
                reinvest_l,
                ..
            },
            _,
        ) => {
            let ticks = e.ticks.as_ref().ok_or("карта тиков не прочитана")?;
            let s = ElasticState {
                sqrt_p: *sqrt_price_x96,
                current_tick: *tick,
                base_l: *base_l,
                reinvest_l: *reinvest_l,
                fee_units: NO_FEE,
                ticks,
                v1: e.family.variant == Variant::ElasticV1,
            };
            elastic(&s, zfo, b, buy).map(|(i, o, f)| (pick(i, o), f))
        }
        // Algebra (THENA Fusion, Algebra Integral; решение 205): обмен до границы полосы (quote/depth_band_algebra.rs).
        (
            PoolState::Algebra {
                sqrt_price_x96,
                tick,
                liquidity,
                ..
            },
            _,
        ) => {
            let ticks = e.ticks.as_ref().ok_or("карта тиков не прочитана")?;
            let s = super::super::math::swap_algebra::AlgebraState {
                sqrt_price: *sqrt_price_x96,
                tick: *tick,
                liquidity: *liquidity,
                fee: NO_FEE,
                ticks,
                next: super::pool_quote::algebra_next(e.family.proto),
            };
            super::depth_band_algebra::algebra(&s, zfo, b, buy).map(|(i, o, f)| (pick(i, o), f))
        }
        // DODO PMM (решение 86b): глубина по средней цене исполнения в полосе (quote/pmm_depth.rs), без комиссии.
        (PoolState::Pmm(d), PoolKind::Pmm) => {
            let state = super::super::math::swap_dodo::PmmState {
                i: d.i,
                k: d.k,
                b: d.b,
                q: d.q,
                b0: d.b0,
                q0: d.q0,
                r: super::super::pools::load_dodo::rstate(d.r).ok_or("R вне 0..=2")?,
            };
            let base = if d.base_is_token0 {
                e.pool.token0
            } else {
                e.pool.token1
            };
            let coin_is_base = e.coin == base;
            super::pmm_depth::side(&state, coin_is_base, b, buy)
                .ok_or_else(|| "расчёт глубины PMM невозможен".to_string())
        }
        // Solidly (Topaz v2, THENA V1; решения 80, 85), без комиссии (чистый объём, решение 73): volatile — та же
        // формула пары, что V2 (x·y=k); stable — средняя цена исполнения в полосе (quote/solidly_depth.rs).
        (
            PoolState::Solidly {
                reserve0,
                reserve1,
                stable,
                decimals0,
                decimals1,
                ..
            },
            _,
        ) => {
            // Решение 483: у Ramses Legacy выход пары на 1 меньше формулы (Pair.sol L457) — на продаже (сумма —
            // выход пары) тот же variant_out, что в pool_quote.rs.
            let minus_one = |r: (U256, bool)| -> Result<(U256, bool), String> {
                if buy || r.0.is_zero() {
                    return Ok(r);
                }
                let out = solidly_v1_math::variant_out(e.family.variant, r.0)
                    .ok_or("выход пары меньше 1")?;
                Ok((out, r.1))
            };
            if *stable {
                // Монета — token0? (solidly_depth сам раскладывает монету/встречный по направлению buy/sell).
                let coin_is_token0 = e.coin == e.pool.token0;
                super::solidly_depth::side(
                    *reserve0,
                    *reserve1,
                    *decimals0,
                    *decimals1,
                    coin_is_token0,
                    b,
                    buy,
                )
                .ok_or_else(|| "расчёт глубины стабильного пула Solidly невозможен".to_string())
                .and_then(minus_one)
            } else {
                let (r_in, r_out) = if zfo {
                    (*reserve0, *reserve1)
                } else {
                    (*reserve1, *reserve0)
                };
                let grow = if buy {
                    (1.0 + b).sqrt() - 1.0
                } else {
                    1.0 / (1.0 - b).sqrt() - 1.0
                };
                let f = U256::from((grow * SCALE) as u128);
                let eff =
                    r_in.checked_mul(f).ok_or("переполнение резерва")? / U256::from(SCALE as u128);
                minus_one(pair_depth(eff, r_in, r_out, buy))
            }
        }
        // Решение 484: пул Wombat — средняя цена исполнения в полосе по идеальному выходу (quote/wombat_depth.rs).
        (PoolState::StableSwap(d), PoolKind::StableSwap) if d.wombat.is_some() => {
            let (ic, iu) = if e.coin == e.pool.token0 {
                (d.i0, d.i1)
            } else {
                (d.i1, d.i0)
            };
            if d.halted {
                return Err("пул Wombat на паузе".into());
            }
            super::wombat_depth::side(d, (usize::from(ic), usize::from(iu)), b, buy)
                .ok_or_else(|| "расчёт глубины пула Wombat невозможен".to_string())
        }
        // StableSwap (решение 122 F): средняя цена исполнения в полосе (quote/stable_depth.rs), без комиссии.
        (PoolState::StableSwap(d), PoolKind::StableSwap) => {
            let (ic, iu) = if e.coin == e.pool.token0 {
                (d.i0, d.i1)
            } else {
                (d.i1, d.i0)
            };
            if d.halted {
                return Err("пул StableSwap на паузе или выключен".into());
            }
            let t = super::super::math::stableswap_v1::Template {
                amp_precision: d.amp_precision,
                max_rounds: d.max_rounds,
                strict: d.strict,
                pair_d: d.pair_d,
            };
            super::stable_depth::side(
                &d.balances,
                &d.rates,
                super::stable_depth::curve_amp(d),
                (ic as usize, iu as usize),
                b,
                buy,
                &t,
            )
            .ok_or_else(|| "расчёт глубины пула StableSwap невозможен".to_string())
        }
        // PMM от оракула (WooFi, решение 122 F, 3; Metric OMM, решение 285): своей математики нет — глубина по лесенке.
        // Лесенка цен пула (quote/pmm_ladder.rs): покупка — вход котируемой, средняя цена (выход за вход) не ниже цены
        // первой точки /(1 + b); продажа — вход базовой, не ниже ×(1 − b). Ответы квотера — с комиссией пула (глубина
        // чуть занижена против «чистого объёма»). Лесенка кончилась раньше полосы — full = false.
        (PoolState::YieldBasisPool(s),PoolKind::V2)=>{
            let points=s.points(token_in).ok_or("YieldBasis: чужая валюта")?;
            let factor=if buy{1.0/(1.0+b)}else{1.0-b};
            let (i,o,full)=super::pmm_ladder::depth(points,U256::from((factor*1e18) as u128)).ok_or("YieldBasis: недоступны измеренные ступени")?;
            Ok((pick(i,o),full))
        }
        (PoolState::OraclePmm(d), PoolKind::OraclePmm) => {
            let (points, factor) = if buy {
                (&d.ladder_quote_in, 1.0 / (1.0 + b))
            } else {
                (&d.ladder_base_in, 1.0 - b)
            };
            let f = U256::from((factor * 1e18) as u128);
            let (i, o, full) = super::pmm_ladder::depth(points, f)
                .ok_or("лесенки цен пула (WooFi / Metric) нет — глубина не считается")?;
            Ok((pick(i, o), full))
        }
        // TickBin (Maverick V2, решение 122 F): порт расчёта обмена без комиссии до тика границы полосы
        // (quote/tick_bin_spot.rs depth); полоса не дочитана (непрочитанный тик) — full = false.
        (PoolState::TickBin(d), PoolKind::TickBin) => {
            let a_in = token_in == e.pool.token0;
            let n = band_ticks(b, buy).ok_or("полоса вне 0–100%")?;
            let (i, o, full) = super::tick_bin_spot::depth(d, a_in, n, U256::from(u128::MAX))
                .ok_or("расчёт глубины пула Maverick V2 невозможен")?;
            Ok((pick(i, o), full))
        }
        // V2 с усилением (KyberSwap Classic DMM, решение 287): та же формула пары x·y=k, что V2, по ВИРТУАЛЬНЫМ резервам и
        // без комиссии; выход не может дойти до реального резерва — тогда глубина до него и full = false.
        (PoolState::AmpV2(d), PoolKind::AmpV2) => {
            let s = if zfo {
                dmm::Side {
                    r_in: d.reserve0,
                    r_out: d.reserve1,
                    v_in: d.v_reserve0,
                    v_out: d.v_reserve1,
                }
            } else {
                dmm::Side {
                    r_in: d.reserve1,
                    r_out: d.reserve0,
                    v_in: d.v_reserve1,
                    v_out: d.v_reserve0,
                }
            };
            let grow = if buy {
                (1.0 + b).sqrt() - 1.0
            } else {
                1.0 / (1.0 - b).sqrt() - 1.0
            };
            let f = U256::from((grow * SCALE) as u128);
            let eff =
                s.v_in.checked_mul(f).ok_or("переполнение резерва")? / U256::from(SCALE as u128);
            if eff.is_zero() || s.r_out.is_zero() {
                return Ok((U256::ZERO, true));
            }
            match dmm::amount_out(eff, s, U256::ZERO) {
                Some(o) => Ok((pick(eff, o), true)),
                None => {
                    let o = s.r_out - U256::from(1u8);
                    let i = dmm::amount_in(o, s, U256::ZERO)
                        .ok_or("расчёт глубины пула DMM невозможен")?;
                    Ok((pick(i, o), false))
                }
            }
        }
        // Пул в общем хранилище (Balancer V2, решение 289): средняя цена исполнения в полосе по формуле пула без комиссии
        // (quote/vault_depth.rs); весовой — не дальше 30 % баланса входа (предел пула, full = false).
        (PoolState::VaultPool(d), PoolKind::VaultPool) => {
            let (i, j) = if zfo { (d.i0, d.i1) } else { (d.i1, d.i0) };
            super::vault_depth::side(d, i.into(), j.into(), b, buy)
                .ok_or_else(|| "расчёт глубины пула Balancer невозможен".to_string())
        }
        // Crypto-пул Curve (решение 442): средняя цена исполнения в полосе по формуле пула без комиссии
        // (quote/crypto_depth.rs).
        (PoolState::Crypto(d), PoolKind::Crypto) => {
            let (i, j) = if zfo { (d.i0, d.i1) } else { (d.i1, d.i0) };
            super::crypto_depth::side(d, i.into(), j.into(), b, buy)
                .ok_or_else(|| "расчёт глубины crypto-пула Curve невозможен".to_string())
        }
        // Пара с фиктивными резервами (SmarDex, решение 288): оценка — x·y=k по фиктивным резервам без комиссии (формула
        // первого шага обмена пары); выход не доходит до резерва пары — тогда до него и full = false. Деление обмена на два
        // шага по средней цене здесь не учитывается (глубина — оценка для порога, котировку считает math/smardex.rs).
        (PoolState::Fictive(d), PoolKind::Fictive) => {
            let s = super::super::math::smardex_swap::side(d, zfo);
            let grow = if buy {
                (1.0 + b).sqrt() - 1.0
            } else {
                1.0 / (1.0 - b).sqrt() - 1.0
            };
            let f = U256::from((grow * SCALE) as u128);
            let eff = s.fictive_in.checked_mul(f).ok_or("переполнение резерва")?
                / U256::from(SCALE as u128);
            let one = (U256::from(1u8), U256::from(1u8));
            let out = v2::amount_out(eff, s.fictive_in, s.fictive_out, one).unwrap_or(U256::ZERO);
            if out.is_zero() {
                return Ok((U256::ZERO, true));
            }
            if out < s.reserve_out {
                return Ok((pick(eff, out), true));
            }
            let o = s.reserve_out.saturating_sub(U256::from(1u8));
            let i = v2::amount_in(o, s.fictive_in, s.fictive_out, one)
                .ok_or("расчёт глубины пары SmarDex невозможен")?;
            Ok((pick(i, o), false))
        }
        // Bin: до края прочитанных бинов (полосы нет — см. шапку), без комиссии.
        (PoolState::Bin { active_id, .. }, PoolKind::Bin) => {
            let bins = e.bins.as_ref().ok_or("бины не прочитаны")?;
            let zfo_in = token_in == e.pool.token0;
            let r = bin_swap::swap(bins, *active_id, NO_FEE, zfo_in, true, u128::MAX)
                .ok_or("расчёт бинов невозможен")?;
            Ok((pick(r.amount_in, r.amount_out), false))
        }
        // Бины с изменчивой комиссией (Liquidity Book V2.1, решение 290): как Bin — до края прочитанных бинов, без
        // комиссии (база и изменчивая часть обнулены в копии состояния), порт getSwapOut (math/lb_swap.rs).
        (PoolState::LbBin(d), PoolKind::LbBin) => {
            let bins = e.bins.as_ref().ok_or("бины не прочитаны")?;
            let mut nf = (**d).clone();
            nf.base_factor = 0;
            nf.variable_fee_control = 0;
            let for_y = (token_in == e.pool.token0) == d.x_is_token0;
            // Решение 446 (ревью Н2): LB V2.2 — бин сверх MAX_LIQUIDITY_PER_BIN — конец ликвидности полосы (depth_v22);
            // обмен через пул исключён (ревью С1) — глубины нет, причина наружу.
            if let Some(blocked) = &d.swap_blocked {
                return Err(format!("Liquidity Book: {blocked}"));
            }
            let swap = if e.family.proto == super::super::pools::families::Proto::LbV22 {
                super::super::math::lb_swap::depth_v22
            } else {
                super::super::math::lb_swap::swap_out
            };
            let r = swap(&nf, bins, d.block_ts, for_y, u128::MAX)
                .ok_or("расчёт бинов Liquidity Book невозможен")?;
            Ok((
                pick(U256::from(u128::MAX - r.in_left), U256::from(r.out)),
                false,
            ))
        }
        // Пул Fluid DEX T1 (решение 440): x·y=k по мнимым резервам частей без комиссии до сдвига цены на полосу, не
        // дальше реального резерва (quote/fluid_depth.rs).
        (PoolState::SmartColDebt(d), PoolKind::SmartColDebt) => {
            super::fluid_depth::side(d, token_in, b, buy)
        }
        // Скважина Basin (решение 485): средняя цена исполнения в полосе (quote/basin_depth.rs); комиссии нет.
        (PoolState::BasinWell(d), PoolKind::BasinWell) => {
            super::basin_depth::side(d, token_in, b, buy)
        }
        // Пул Shell Protocol Proteus (решение 486): средняя цена исполнения формулой пула в полосе, с комиссией
        // округлением (quote/shell_depth.rs).
        (PoolState::Proteus(d), PoolKind::Proteus) => super::shell_depth::side(d, token_in, b, buy),
        // Ekubo (решение 447): обмен без комиссии до границы полосы в тиках Ekubo (depth_ekubo.rs).
        // Решение 454: направление — в порядке ключа (нативный ETH); у пула с TWAMM — состояние к блоку сделки (первое из
        // двух времён, quote/ekubo_trade_state.rs).
        (PoolState::Ekubo(d), PoolKind::Ekubo) => {
            let kz = !super::super::pools::ekubo_native::is_token1(d, e.pool.token0, token_in);
            let s = super::ekubo_trade_state::at_trade(d)?;
            let s = s.first().ok_or("нет состояния Ekubo к блоку сделки")?;
            super::depth_ekubo::side(s, kz, b, buy)
        }
        _ => Err("вид пула и состояние не совпадают".into()),
    }
}

/// Объём пары x·y=k внутри выбранной полосы изменения цены.
fn xy_band(r_in: U256, r_out: U256, b: f64, buy: bool) -> Result<(U256,bool),String> {
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
            Ok(pair_depth(eff, r_in, r_out, buy))
}

/// Пара x·y=k (V2, Solidly volatile) без комиссии: (сумма во встречном токене, true). Решение 205: пустой пул
/// (резерв 0) или резерв так мал, что сдвиг цены на полосу меньше единицы токена (eff = 0), — глубина 0, а не
/// ошибка: раньше «расчёт пары V2 / Solidly невозможен» исключал сторону с записью Warn в журнал (06.11 —
/// uniswap-v2-bsc, mdex_bsc, pancakeswap_v2, thena), хотя такой пул просто пуст и порог глубины его и так не пустит.
/// Ошибок нет.
fn pair_depth(eff: U256, r_in: U256, r_out: U256, buy: bool) -> (U256, bool) {
    let none = (U256::from(1u8), U256::from(1u8));
    let out = v2::amount_out(eff, r_in, r_out, none).unwrap_or(U256::ZERO);
    let (i, o) = if out.is_zero() {
        (U256::ZERO, U256::ZERO)
    } else {
        (eff, out)
    };
    (if buy { i } else { o }, true)
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

/// Elastic: обмен до цены на границе полосы (как cl, но math/swap_elastic). Возвращает: (отдано, получено, дошли
/// до границы). Ошибки (текст): расчёт.
fn elastic(s: &ElasticState, zfo: bool, b: f64, buy: bool) -> Result<(U256, U256, bool), String> {
    let d = band_ticks(b, buy).ok_or("полоса вне 0–100%")?;
    if d == 0 {
        return Ok((U256::ZERO, U256::ZERO, true));
    }
    let edge = if zfo {
        s.current_tick - d
    } else {
        s.current_tick + d
    }
    .clamp(MIN_TICK + 1, MAX_TICK - 1);
    let limit = sqrt_ratio_at_tick(edge).ok_or("цена границы полосы вне диапазона TickMath")?;
    let (r, reached) = swap_elastic::swap_to(s, zfo, true, U256::from(u128::MAX), limit)
        .map_err(str::to_string)?;
    Ok((r.amount_in, r.amount_out, reached))
}
