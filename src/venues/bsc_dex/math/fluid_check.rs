//! Файл: venues/bsc_dex/math/fluid_check.rs
//! Что делает: решение 440 — проверки обмена в пуле Fluid DEX T1 после расчёта выхода (math/fluid_swap.rs):
//! - пределы слоя ликвидности: выход залога ≤ «можно снять», выход долга ≤ «можно занять» (приведённые единицы) — как
//!   официальный резолвер в estimateSwapIn (пределы — его же getDexLimits: учитывают пределы снятия/займа, балансы и
//!   предел загрузки слоя ликвидности и пула);
//! - скачок цены за обмен: у пула с VERSION() — порт _updateOracle / _priceDiffCheck пула (сохранённые цены и время
//!   последнего обмена из слова dexVariables, время — блока чтения); у пула без VERSION() (развёрнут до 1.1.0, его
//!   исходника в официальном репозитории нет) — та же проверка, что делает официальный резолвер для таких пулов
//!   (_priceMoveExceedsLimit, ветка catch): старая цена — текущая цена части с бо́льшим входом.
//!
//! Итог — выход обмена или None (пул или слой ликвидности откатили бы обмен; резолвер вернул бы 0).
//! Кто вызывает: quote/pool_quote.rs (ветка SmartColDebt), тесты.
//! От чего зависит: math/fluid_swap.rs, core/types (SmartColDebtData), alloy-primitives.
//! Настройки: нет.
//!
//! Источник: github.com/Instadapp/fluid-contracts-public@404d73f91e6806f881f8dabeb30494f93f9e99b7 —
//! contracts/protocols/dex/poolT1/coreModule/helpers/coreHelpers.sol L814-L828 (_priceDiffCheck), L830-L872
//! (_updateOracle); common/constantVariables.sol L25-L26 (DEFAULT_EXPONENT_SIZE / MASK), L47-L48 (X33, X40), L58
//! (EIGHT_DECIMALS), L67-L68 (ORACLE_PRECISION, ORACLE_LIMIT); contracts/periphery/resolvers/dexReserves/main.sol
//! L713-L731 (_priceMoveExceedsLimit), L839-L866 (пределы в estimateSwapIn), L868-L898 (старая и новая цена).

use alloy_primitives::{I256, U256};

use crate::core::types::SmartColDebtData;

use super::fluid_swap::{self, Out};

/// ORACLE_PRECISION = 1e18 (constantVariables.sol L67).
const E18: u128 = 1_000_000_000_000_000_000;
/// ORACLE_LIMIT = 5e16 — 5 % (L68).
const LIMIT: u128 = 50_000_000_000_000_000;
/// EIGHT_DECIMALS = 1e8 (L58).
const EIGHT: u128 = 100_000_000;
/// Точность цены 1e27 (резолвер L871-L897).
const E27: u128 = 1_000_000_000_000_000_000_000_000_000;

/// Число BigNumber пула: (x >> 8) << (x & 0xFF) (DEFAULT_EXPONENT_SIZE = 8, MASK = 0xFF). Ошибок нет.
fn big(x: U256) -> U256 {
    let exp = (x & U256::from(0xffu8)).to::<usize>();
    (x >> 8usize).wrapping_shl(exp)
}

/// Поле слова: (w >> shift) & (2^bits − 1). Ошибок нет.
fn field(w: U256, shift: usize, bits: usize) -> U256 {
    let one = U256::from(1u8);
    (w >> shift) & ((one << bits) - one)
}

/// _priceDiffCheck (L814-L828, unchecked): новая цена в пределах старая·(1 ± 5 %). true — проходит.
fn diff_ok(old: U256, new: U256) -> bool {
    let base = U256::from(E18);
    let upper = old.wrapping_mul(base + U256::from(LIMIT)) / base;
    let lower = old.wrapping_mul(base - U256::from(LIMIT)) / base;
    (lower..=upper).contains(&new)
}

/// _updateOracle (L830-L872): проверки скачка цены у пула с VERSION(). Принимает: состояние, новую цену. Возвращает:
/// true — проходит (время блока раньше записанного — откат, false).
fn oracle_ok(d: &SmartColDebtData, price: U256) -> bool {
    let w = d.dex_variables;
    let last_ts = field(w, 121, 33);
    let now = U256::from(d.block_ts);
    if now < last_ts {
        return false;
    }
    if now > last_ts {
        return diff_ok(big(field(w, 41, 40)), price);
    }
    // Второй обмен в ту же секунду: центральная цена — в пределах ±1e-8 от сохранённой, цена — к цене до прошлого обмена.
    let old_center = big(field(w, 81, 40));
    let e8 = U256::from(EIGHT);
    let one = U256::from(1u8);
    let low = (e8 - one).wrapping_mul(old_center) / e8;
    let high = (e8 + one).wrapping_mul(old_center) / e8;
    if !(low..=high).contains(&d.center_price) {
        return false;
    }
    diff_ok(big(field(w, 1, 40)), price)
}

/// Старая и новая цена так, как их считает резолвер (L868-L898): часть с бо́льшим входом, вход ДО доли протокола.
/// None — деление на ноль / вычитание ниже нуля (резолвер откатил бы — выхода нет).
fn resolver_prices(o: &Out, swap0to1: bool) -> Option<(U256, U256)> {
    let e27 = U256::from(E27);
    let (p, inp, out) = if o.in_col > o.in_debt {
        (o.col, o.in_col, o.out_col)
    } else {
        (o.debt, o.in_debt, o.out_debt)
    };
    let rest = p.out_imag.checked_sub(out)?;
    let grown = p.in_imag.checked_add(inp)?;
    if swap0to1 {
        let old = p.out_imag.checked_mul(e27)?.checked_div(p.in_imag)?;
        Some((old, rest.checked_mul(e27)?.checked_div(grown)?))
    } else {
        let old = p.in_imag.checked_mul(e27)?.checked_div(p.out_imag)?;
        Some((old, grown.checked_mul(e27)?.checked_div(rest)?))
    }
}

/// Скачок цены у пула без VERSION() — ветка catch _priceMoveExceedsLimit резолвера (L722-L730): 1e18 − old·1e18/new
/// в пределах ±5e16. true — проходит.
fn legacy_ok(old: U256, new: U256) -> bool {
    if new.is_zero() {
        return false;
    }
    let base = U256::from(E18);
    let Some(ratio) = old.checked_mul(base).map(|v| v / new) else {
        return false;
    };
    let Some(diff) = I256::from_raw(base).checked_sub(I256::from_raw(ratio)) else {
        return false;
    };
    let lim = I256::from_raw(U256::from(LIMIT));
    (-lim..=lim).contains(&diff)
}

/// Приведённый предел (единицы токена → приведённые): v·num/den. None — деление на ноль. Ошибок нет.
fn scaled(v: U256, num: U256, den: U256) -> Option<U256> {
    v.checked_mul(num)?.checked_div(den)
}

/// Обмен с точным входом со всеми проверками. Принимает: состояние, направление (token0 пула → token1), вход в
/// единицах токена. Возвращает: выход в единицах токена или None — обмен откатился бы (пул, пределы слоя
/// ликвидности, скачок цены).
pub fn quote_in(d: &SmartColDebtData, swap0to1: bool, amount_in: U256) -> Option<U256> {
    let o = fluid_swap::swap_in(d, swap0to1, amount_in)?;
    let (w, b, num, den) = if swap0to1 {
        (d.withdrawable1, d.borrowable1, d.num1, d.den1)
    } else {
        (d.withdrawable0, d.borrowable0, d.num0, d.den0)
    };
    if o.out_debt > scaled(b, num, den)? || o.out_col > scaled(w, num, den)? {
        return None;
    }
    let ok = if d.versioned {
        oracle_ok(d, o.price)
    } else {
        let (old, new) = resolver_prices(&o, swap0to1)?;
        legacy_ok(old, new)
    };
    ok.then_some(o.amount_out)
}
