//! Файл: venues/bsc_dex/quote/bound_rate.rs
//! Что делает: решение 372 — строгий верх средней отдачи ОДНОГО пула в одну сторону («r_e» проекта
//! docs/notes/t162-proven-sieve-2026-11-17.md, раздел 2): для любого входа x > 0 выход обмена модели бота (quote/pool_quote.rs
//! swap) не больше r_e·x (в минимальных единицах токенов). Число — дробь (U256/U256), в Decimal переводится с округлением
//! ВВЕРХ (ceil_dec) и перемножается с округлением вверх (mul_up) — ошибка округления только в сторону верха.
//! Виды (доказательство — заметка, раздел 2, таблица «вид пула»):
//! - V2 (x·y = k, getAmountOut с округлением вниз): выход = ⌊x·γ·R_out/(R_in + x·γ)⌋ ≤ γ·R_out/R_in·x; γ — та же доля
//!   комиссии, что у обмена (pool_quote::v2_fee_pct); комиссия не задана — без комиссии (верх слабее, но строгий);
//! - V3 (PancakeSwap/Uniswap V3 и форки): цена в обмене только ухудшается (TickMath монотонна), комиссия берётся с входа
//!   с округлением вверх (SwapMath.computeSwapStep feeAmount mulDivRoundingUp) ⇒ выход ≤ P·(1 − fee/10⁶)·x, P —
//!   текущая цена sqrtPriceX96² / 2^192 (token1 за token0) или обратная;
//! - Algebra: та же монотонность цены; комиссия плагина может быть иной, поэтому — БЕЗ комиссии: выход ≤ P·x;
//! - Solidly волатильный (x·y = k на сырых резервах, вход после комиссии пула вниз до целого): выход ≤ R_out/R_in·x —
//!   без комиссии (комиссия net_in округляется вниз, на малом входе доля комиссии меньше номинальной).
//!
//! Прочие виды (стабильные инварианты, бины, PMM, TickBin, Fictive, Vault, AmpV2, CL/Bin менеджеров V4/Infinity) и пулы
//! с крючком — Err: метод не действует, пока нет теста-эталона на записи (заметка, раздел 3).
//! Кто вызывает: quote/bound.rs (верх курса монеты и цикла).
//! От чего зависит: math/full_math.rs, math/v2.rs, quote/pool_quote.rs (v2_fee_pct), pools/registry.rs, core/types.
//! Настройки: через pool_quote::v2_fee_pct — dex.v2_fee_pct [вид пула V2].

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, PoolState};

use super::super::ctx::Ctx;
use super::super::math::full_math::{mul_div, mul_div_up};
use super::super::math::v2;
use super::super::pools::registry::Entry;
use super::pool_quote;

/// Дробь num/den (минимальные единицы выхода за единицу входа).
#[derive(Debug, Clone, Copy)]
pub struct Ratio {
    pub num: U256,
    pub den: U256,
}

/// PIPS_DENOMINATOR = 1e6 — единицы комиссии V3 (Uniswap v3-core UniswapV3Pool.sol: fee в сотых долях бипса).
const PIPS: u64 = 1_000_000;

/// Что делает: верх отдачи пула `e` при входе токеном `token_in` (см. шапку). Принимает: окружение (настройка комиссии
/// V2), запись пула, токен входа. Возвращает: дробь; Err — вид пула без строгого верха или крючок (текст причины —
/// «метод не действует»), пустой пул (нулевой резерв/цена).
pub fn rate(ctx: &Ctx, e: &Entry, token_in: Address) -> Result<Ratio, String> {
    let zfo = token_in == e.pool.token0;
    if e.pool.hooks.as_ref().is_some_and(|h| !h.address.is_zero()) {
        return Err(format!("{}: пул с крючком", e.family.title));
    }
    match (&e.pool.state, e.pool.kind) {
        (PoolState::Memebox(s), PoolKind::V2) => {
            let i=usize::from(!zfo);
            pair(s.reserves[1-i],U256::ONE,s.reserves[i],U256::ONE)
        }
        (PoolState::CandySwap(s), PoolKind::V2) => {
            let i=usize::from(!zfo);
            let rin=if s.balances[i]>s.reserves[i] {U256::ONE} else {s.reserves[i]};
            pair(s.reserves[1-i],U256::ONE,rin,U256::ONE)
        }
        (PoolState::EtherVista(s), PoolKind::V2) => {
            // Без фиксированной платы это верх курса для любого размера; её точная величина входит в quote.
            let (rin, rout) = if zfo { (s.reserve0,s.reserve1) } else { (s.reserve1,s.reserve0) };
            let backing = if zfo { s.balance0 } else { s.balance1 };
            let floor_in = if backing > rin { U256::ONE } else { rin };
            pair(rout, U256::ONE, floor_in, U256::ONE)
        }
        (
            PoolState::DeltaSwap {
                reserve0, reserve1, ..
            },
            PoolKind::V2,
        ) => {
            let (rin, rout) = if zfo {
                (*reserve0, *reserve1)
            } else {
                (*reserve1, *reserve0)
            };
            pair(rout, U256::ONE, rin, U256::ONE)
        }
        (PoolState::V2 { reserve0, reserve1 } | PoolState::Oasis { reserve0, reserve1, .. }, PoolKind::V2) => {
            let (r_in, r_out) = if zfo {
                (*reserve0, *reserve1)
            } else {
                (*reserve1, *reserve0)
            };
            // Комиссия — та же, что у обмена; не задана или вне 0–100 % — без комиссии (верх строгий, слабее).
            let fee = pool_quote::v2_fee_pct(ctx, e)
                .ok()
                .and_then(v2::fee_ratio)
                .unwrap_or((U256::from(1u8), U256::from(1u8)));
            pair(r_out, fee.0, r_in, fee.1)
        }
        (PoolState::V3 { sqrt_price_x96, .. }, PoolKind::V3) => {
            let fee = e.pool.fee_pips.map_or(0, u64::from).min(PIPS);
            sqrt_rate(
                *sqrt_price_x96,
                zfo,
                U256::from(PIPS - fee),
                U256::from(PIPS),
            )
        }
        (PoolState::Algebra { sqrt_price_x96, .. }, _) => {
            sqrt_rate(*sqrt_price_x96, zfo, U256::from(1u8), U256::from(1u8))
        }
        (
            PoolState::Solidly {
                reserve0,
                reserve1,
                stable: false,
                ..
            },
            _,
        ) => {
            let (r_in, r_out) = if zfo {
                (*reserve0, *reserve1)
            } else {
                (*reserve1, *reserve0)
            };
            pair(r_out, U256::from(1u8), r_in, U256::from(1u8))
        }
        _ => Err(format!(
            "{}: вид пула {:?} — строгого верха без записи-эталона нет",
            e.family.title, e.pool.kind
        )),
    }
}

/// (a·b)/(c·d) без переполнения: множители проверяются. Err — нулевой знаменатель (пустой пул) или переполнение.
fn pair(a: U256, b: U256, c: U256, d: U256) -> Result<Ratio, String> {
    let num = a.checked_mul(b).ok_or("переполнение верха")?;
    let den = c.checked_mul(d).ok_or("переполнение верха")?;
    if den.is_zero() || num.is_zero() {
        return Err("пустой пул".into());
    }
    Ok(Ratio { num, den })
}

/// Цена по sqrtPriceX96 с долей f_num/f_den: token1 за token0 — sqrtP²/2^192 (числитель вверх), token0 за token1 —
/// 2^192/sqrtP² (знаменатель вниз). Err — нулевая цена, переполнение.
fn sqrt_rate(sqrt: U256, zfo: bool, f_num: U256, f_den: U256) -> Result<Ratio, String> {
    let q96 = U256::from(1u8) << 96;
    let (num, den) = if zfo {
        (mul_div_up(sqrt, sqrt, q96).ok_or("переполнение цены")?, q96)
    } else {
        (q96, mul_div(sqrt, sqrt, q96).ok_or("переполнение цены")?)
    };
    pair(num, f_num, den, f_den)
}

/// Что делает: дробь → Decimal с округлением ВВЕРХ (наибольший масштаб ≤ 28, при котором мантисса помещается в 96 бит).
/// Принимает: дробь. Возвращает: Decimal ≥ num/den; None — значение больше ~7,9·10^28.
pub fn ceil_dec(r: Ratio) -> Option<Decimal> {
    let max = U256::from(u128::MAX >> 32);
    if r.den.is_zero() {
        return None;
    }
    // Начальный масштаб — по числу цифр целой части (обычно первая же попытка помещается).
    let (mut ip, mut digits) = (r.num / r.den, 0u32);
    while !ip.is_zero() {
        ip /= U256::from(10u8);
        digits += 1;
    }
    let mut scale = Decimal::MAX_SCALE.saturating_sub(digits);
    loop {
        // Решение 410 (t195): 10^scale — из таблицы math/ten.rs (тот же pow).
        let m = mul_div_up(r.num, super::super::math::ten::pow(u64::from(scale)), r.den)?;
        if m <= max {
            return Decimal::try_from_i128_with_scale(i128::try_from(m).ok()?, scale).ok();
        }
        if scale == 0 {
            return None;
        }
        scale -= 1;
    }
}

/// Что делает: a·b с округлением вверх (к произведению прибавляется единица последнего знака результата — ошибка
/// округления Decimal не больше половины её). Принимает: неотрицательные a, b. Возвращает: верх; None — переполнение.
pub fn mul_up(a: Decimal, b: Decimal) -> Option<Decimal> {
    let p = a.checked_mul(b)?;
    p.checked_add(Decimal::new(1, p.scale()))
}

/// Что делает: 10^k как множитель decimals (k может быть отрицательным). Возвращает: None — |k| > 28. Ошибок нет.
pub fn pow10(k: i32) -> Option<Decimal> {
    let n = k.unsigned_abs();
    if n > Decimal::MAX_SCALE {
        return None;
    }
    if k >= 0 {
        Decimal::try_from_i128_with_scale(10i128.checked_pow(n)?, 0).ok()
    } else {
        Some(Decimal::new(1, n))
    }
}
