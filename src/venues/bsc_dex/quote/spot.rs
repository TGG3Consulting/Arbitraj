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
        PoolState::V2 { reserve0, reserve1 } => {
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
