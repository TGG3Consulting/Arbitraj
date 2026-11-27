//! Файл: venues/bsc_dex/quote/spot.rs
//! Что делает: сколько монет стоит сумма во встречном токене по ТЕКУЩЕЙ цене пула — без обмена (решение 73, техлид:
//! проба продажи для сверки с квотером не должна зависеть от того, можно ли купить на эту сумму: у CL-пула с
//! ликвидностью только с одной стороны цены продажа глубокая, а покупка невозможна).
//! Цена: V2 — резервы (y/x); V3, Infinity CL, Uniswap V4 — sqrtPriceX96 (цена токена0 в токене1 = (sqrtPriceX96 / 2^96)²);
//! Infinity Bin — цена активного бина (128.128, сколько Y за 1 X).
//! Кто вызывает: check/quoter_verify.rs (проба продажи).
//! От чего зависит: math/full_math.rs (mul_div), math/bin_price.rs, pools/registry.rs.
//! Настройки: нет.
//!
//! sqrtPriceX96 = √(token1/token0)·2^96 — Uniswap v3-core contracts/libraries/TickMath.sol; цена бина —
//! infinity-core src/pool-bin/libraries/PriceHelper.sol (math/bin_price.rs).

use alloy_primitives::U256;

use crate::core::types::PoolState;

use super::super::math::bin_price::{self, SCALE_OFFSET};
use super::super::math::crypto_swap;
use super::super::math::full_math::mul_div;
use super::super::math::swap_dodo::{self, PmmState};
use super::super::pools::load_dodo::rstate;
use super::super::pools::registry::Entry;

/// 2^96 — масштаб sqrtPriceX96 (протокол, не настройка).
fn q96() -> U256 {
    U256::from(1u8) << 96
}

/// Монеты на сумму `amount` встречного токена по текущей цене пула (минимальные единицы). None — цены нет (нулевые
/// резервы или цена, бин без binStep) или переполнение. Ошибок нет.
pub fn coins_for(e: &Entry, amount: U256) -> Option<U256> {
    let coin0 = e.coin == e.pool.token0;
    match &e.pool.state {
        PoolState::UmbraeDamm(s) => {
            let i = s.tokens.iter().position(|&t| t == e.coin)?;
            mul_div(amount, s.reserves[i], s.reserves[1-i])
        }
        PoolState::Antfarm(book) => { let s=&book.market; let i=usize::from(e.coin==s.tokens[1]); mul_div(amount,s.reserves[i],s.reserves[1-i]) }
        PoolState::Temple(s) => {
            if e.coin==s.stable || e.usdt==s.stable {
                let i=usize::from(e.coin==s.stable);
                mul_div(amount,s.reserves[i],s.reserves[1-i])
            } else {
                let [n,d]=s.ratio?;
                if e.coin==s.temple {mul_div(amount,d,n)} else {mul_div(amount,n,d)}
            }
        }
        PoolState::Carbon(s) => super::super::math::carbon_depth::coins(s,e.coin,amount),
        PoolState::MintClub(s) => super::super::math::mint_club_depth::spot(s,e.coin,amount),
        PoolState::Virtuals(s) => {let c=usize::from(e.coin==s.asset); mul_div(amount,s.reserves[c],s.reserves[1-c])}
        PoolState::Memebox(s) => {
            let i=usize::from(!coin0);
            mul_div(amount,s.reserves[i],s.reserves[1-i])
        }
        PoolState::ToonSwap(s) => { let i=usize::from(!coin0); mul_div(amount,s.reserves[i],s.reserves[1-i]) }
        PoolState::CandySwap(s) => {
            let i=usize::from(!coin0);
            mul_div(amount,s.reserves[i],s.reserves[1-i])
        }
        PoolState::EtherVista(s) => {
            let (rc, ru) = if coin0 { (s.reserve0,s.reserve1) } else { (s.reserve1,s.reserve0) };
            mul_div(amount, rc, ru)
        }
        PoolState::V2 { reserve0, reserve1 } | PoolState::Oasis { reserve0, reserve1, .. } | PoolState::DeltaSwap { reserve0, reserve1, .. } => {
            let (rc, ru) = if coin0 {
                (*reserve0, *reserve1)
            } else {
                (*reserve1, *reserve0)
            };
            (!ru.is_zero()).then(|| mul_div(amount, rc, ru)).flatten()
        }
        // Solidly (Topaz v2, THENA V1; решения 80, 85): цена для СИЗИНГА пробы продажи (решение 73.19) — отношение
        // резервов (как V2). Для волатильного пула это точная спот-цена; для стабильного — оценка по резервам (у
        // стабильной кривой маржинальная цена иная, но здесь нужен лишь размер пробы, а не сверяемая величина — её
        // считают локальная математика и квотер на одной и той же сумме). decimals в отношении резервов сокращаются.
        PoolState::Solidly {
            reserve0, reserve1, ..
        } => {
            let (rc, ru) = if coin0 {
                (*reserve0, *reserve1)
            } else {
                (*reserve1, *reserve0)
            };
            (!ru.is_zero()).then(|| mul_div(amount, rc, ru)).flatten()
        }
        // StableSwap (решение 122 F): цена для СИЗИНГА пробы (решение 73.19) — 1 : 1 в нормализованных 18 знаках
        // (rates пула): монет = сумма · rate(встречного) / rate(монеты). Это оценка размера пробы, а не сверяемая
        // величина — её считают локальная математика и квотер пула на одной и той же сумме.
        PoolState::StableSwap(d) => {
            let (ic, iu) = if coin0 { (d.i0, d.i1) } else { (d.i1, d.i0) };
            let rc = *d.rates.get(ic as usize)?;
            let ru = *d.rates.get(iu as usize)?;
            (!rc.is_zero()).then(|| mul_div(amount, ru, rc)).flatten()
        }
        // PMM от оракула (WooFi, решение 122 F, 3): своей математики нет (развёрнутый код закрыт). Лесенка цен пула: монет за `amount` встречного — по средней цене первой точки «котируемая → базовая».
        PoolState::OraclePmm(d) if matches!(e.family.proto,super::super::pools::families::Proto::Elfomo|super::super::pools::families::Proto::Lunar|super::super::pools::families::Proto::BaiBai) => {
            if let Some(p) = d.ladder_base_in.first() {
                return mul_div(amount, p.amount_in, p.amount_out);
            }
            let p = d.ladder_quote_in.first()?;
            mul_div(amount, p.amount_out, p.amount_in)
        }
        PoolState::OraclePmm(d) => {
            let p = d.ladder_quote_in.first()?;
            (!p.amount_in.is_zero())
                .then(|| mul_div(amount, p.amount_out, p.amount_in))
                .flatten()
        }
        // TickBin (Maverick V2, решение 122 F): цена активного тика по его резервам (официальный TickMath
        // getTickSqrtPriceAndL, math/maverick_tick_math.rs) в D18-масштабе пула: цена tokenA в tokenB = sqrtPrice²/1e18
        // (D18 → единицы токенов — множители масштаба). Оценка размера пробы (решение 73.19), не сверяемая величина.
        PoolState::TickBin(d) => super::tick_bin_spot::coins_for(d, coin0, amount),
        // Пара с фиктивными резервами (SmarDex, решение 288): цена для сизинга пробы — отношение фиктивных резервов (по ним
        // формула x·y=k первого шага; оценка размера пробы, решение 73.19, не сверяемая величина).
        PoolState::Fictive(d) => {
            let (fc, fu) = if coin0 { (d.fictive0, d.fictive1) } else { (d.fictive1, d.fictive0) };
            (!fu.is_zero()).then(|| mul_div(amount, fc, fu)).flatten()
        }
        // Пул в общем хранилище (Balancer V2, решение 289): цена малой пробы (vault_depth::PROBE_DIV — доля баланса встречного) по формуле
        // пула без комиссии (math/balancer_math.rs) — оценка размера пробы (решение 73.19), не сверяемая величина.
        PoolState::VaultPool(d) => {
            let (ic, iu) = if coin0 { (d.i0, d.i1) } else { (d.i1, d.i0) };
            let mut nf = (**d).clone();
            nf.fee = U256::ZERO;
            let probe = *d.balances.get(iu as usize)? / U256::from(super::vault_depth::PROBE_DIV);
            let out = super::super::math::balancer_math::amount_out(&nf, iu.into(), ic.into(), probe)?;
            (!probe.is_zero()).then(|| mul_div(amount, out, probe)).flatten()
        }
        // Бины с изменчивой комиссией (Liquidity Book V2.1, решение 290): цена активного бина — сколько Y за 1 X, ·2^128
        // (PriceHelper.getPriceFromId); X — token0 записи, если x_is_token0. Оценка размера пробы (решение 73.19).
        PoolState::LbBin(d) => {
            let price = bin_price::price_from_id(d.active_id, d.bin_step)?;
            let one = U256::from(1u8) << SCALE_OFFSET;
            // Монета — X: монет = сумма(Y) / цена; монета — Y: сумма(X) · цена.
            if coin0 == d.x_is_token0 {
                (!price.is_zero()).then(|| mul_div(amount, one, price)).flatten()
            } else {
                mul_div(amount, price, one)
            }
        }
        // Crypto-пул Curve (решение 442): по внутренней цене пула price_scale и множителям точности
        // (math/crypto_swap.rs spot) — оценка размера пробы (решение 73.19), не сверяемая величина.
        PoolState::Crypto(d) => {
            let (ic, iu) = if coin0 { (d.i0, d.i1) } else { (d.i1, d.i0) };
            crypto_swap::spot(d, ic.into(), iu.into(), amount)
        }
        // Ekubo (решение 447): цена token0 в token1 = (sqrtRatio 64.128 / 2^128)² (math/ekubo_sqrt.rs to_fixed) — оценка
        // размера пробы (решение 73.19), не сверяемая величина.
        // Решение 454: монета — token0 КЛЮЧА (нативный ETH — token0 ключа, pools/ekubo_native.rs).
        PoolState::Ekubo(d) => {
            let r = super::super::math::ekubo_sqrt::to_fixed(d.sqrt_ratio);
            let one = U256::from(1u8) << 128usize;
            if !super::super::pools::ekubo_native::is_token1(d, e.pool.token0, e.coin) {
                mul_div(mul_div(amount, one, r)?, one, r)
            } else {
                mul_div(mul_div(amount, r, one)?, r, one)
            }
        }
        // V2 с усилением (KyberSwap Classic DMM, решение 287): спот-цена — отношение ВИРТУАЛЬНЫХ резервов (по ним формула).
        PoolState::AmpV2(d) => {
            let (vc, vu) = if coin0 {
                (d.v_reserve0, d.v_reserve1)
            } else {
                (d.v_reserve1, d.v_reserve0)
            };
            (!vu.is_zero()).then(|| mul_div(amount, vc, vu)).flatten()
        }
        // Пул Fluid DEX T1 (решение 440): отношение мнимых резервов основной части (quote/fluid_depth.rs) — оценка размера
        // пробы (решение 73.19), не сверяемая величина.
        PoolState::SmartColDebt(d) => super::fluid_depth::spot(d, e.coin, amount),
        // Скважина Basin (решение 485): оценка размера пробы (quote/basin_depth.rs), не сверяемая величина.
        PoolState::BasinWell(d) => super::basin_depth::spot(d, e.coin, amount),
        // Пул Shell Protocol Proteus (решение 486): спот малой пробы формулой пула (quote/shell_depth.rs) — оценка
        // размера пробы, не сверяемая величина.
        PoolState::Proteus(d) => super::shell_depth::spot(d, e.coin, amount),
        PoolState::V3 { sqrt_price_x96, .. }
        | PoolState::Cl { sqrt_price_x96, .. }
        | PoolState::Elastic { sqrt_price_x96, .. }
        // Algebra (THENA Fusion, решения 80, 81): цена из sqrtPriceX96, как у V3/CL.
        | PoolState::Algebra { sqrt_price_x96, .. } => {
            let s = *sqrt_price_x96;
            if s.is_zero() {
                return None;
            }
            // Цена токена0 в токене1 = (s/2^96)². Монета — токен0: монет = сумма / цена; токен1: сумма · цена.
            // Двумя делениями (по одному множителю s/2^96) — без переполнения 256 бит.
            if coin0 {
                mul_div(mul_div(amount, q96(), s)?, q96(), s)
            } else {
                mul_div(mul_div(amount, s, q96())?, s, q96())
            }
        }
        PoolState::Bin { active_id, .. } => {
            let price = bin_price::price_from_id(*active_id, e.pool.bin_step?)?;
            let one = U256::from(1u8) << SCALE_OFFSET;
            // price — сколько Y (токен1) за 1 X (токен0), ·2^128.
            if coin0 {
                (!price.is_zero())
                    .then(|| mul_div(amount, one, price))
                    .flatten()
            } else {
                mul_div(amount, price, one)
            }
        }
        // DODO PMM (решение 86b): средняя цена кривой getMidPrice (quote за base, 18 знаков). Монета — база:
        // монет = сумма(quote) · 1e18 / mid; монета — котируемая: монет = сумма(base) · mid / 1e18.
        PoolState::Pmm(d) => {
            let state = PmmState {
                i: d.i,
                k: d.k,
                b: d.b,
                q: d.q,
                b0: d.b0,
                q0: d.q0,
                r: rstate(d.r)?,
            };
            let mid = swap_dodo::get_mid_price(&state)?;
            if mid.is_zero() {
                return None;
            }
            let one = super::super::math::dodo_math::one();
            let base = if d.base_is_token0 {
                e.pool.token0
            } else {
                e.pool.token1
            };
            // coin — база: встречный токен котируемый, монет(base) = сумма(quote)·1e18/mid; иначе наоборот.
            if e.coin == base {
                mul_div(amount, one, mid)
            } else {
                mul_div(amount, mid, one)
            }
        }
    }
}
