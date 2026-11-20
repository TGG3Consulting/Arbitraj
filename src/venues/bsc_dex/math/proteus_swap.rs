//! Файл: venues/bsc_dex/math/proteus_swap.rs
//! Что делает: решение 486 — обмен Proteus с точным входом: swapGivenInputAmount реализации (_swap с FEE_DOWN, комиссия
//! округлением _applyFeeByRounding, проверки _checkAmountWithBalance и _checkBalances) и полный путь учётного контракта
//! Shell (Ocean) для шага исполнителя: вход ERC-20 → единицы учёта (wrapErc20 точной суммой: сумма · 10^(18 − знаков),
//! пыли нет), выход реализации → unwrap (комиссия вывода amount / unwrapFeeDivisor, перевод в знаки токена вниз —
//! остаток уходит владельцу учёта). Нативная сторона (id «Ether») — 18 знаков, без перевода.
//! Кто вызывает: quote/pool_quote.rs, quote/shell_depth.rs, tests/arbitrum_shell.rs.
//! От чего зависит: math/proteus_curve.rs, core/types (ProteusData), alloy-primitives.
//! Настройки: нет (константы — исходника реализации; BASE_FEE и делитель комиссии вывода — из состояния пула).
//!
//! Источник (решение 23): Shell-Protocol/Shell-Protocol@bfbba01 src/proteus/Proteus.sol — INT_MAX L15,
//! MAX_BALANCE_AMOUNT_RATIO L27, FIXED_FEE L31, swapGivenInputAmount L58-L84, _swap L264-L332,
//! _checkAmountWithBalance L753-L760, _checkBalances L768-L778, _applyFeeByRounding L788-L817 (BASE_FEE развёрнутых
//! реализаций: 4000 / 8000 — константа, 800 — константа или immutable v3; читается BASE_FEE()). Реализация получает
//! балансы пула ДО обмена при обоих Ocean, но по-разному: v3 (Ocean 0x96B4f4E4…) зачисляет вход пулу ДО
//! computeOutputAmount, а прокси вычитает вход (0xC32A9fC5… LiquidityPoolProxy.swapGivenInputAmount L49-L61
//! `xBalance -= inputAmount`); v2 (Old Ocean aaa0ccf _computeOutputAmount → _updateBalancesOfPrimitive) зачисляет вход
//! ПОСЛЕ computeOutputAmount, а прокси вход НЕ вычитает (bfbba01 LiquidityPoolProxy.swapGivenInputAmount L66-L77) — у
//! обоих реализации приходит баланс пула до этого обмена. Порт (и квотер) зовут реализацию с прочитанным balanceOfBatch
//! — учёт ещё без входа, т. е. баланс до обмена, — поэтому вход не вычитают. Ocean (aaa0ccf src/ocean/Ocean.sol и
//! Sourcify 42161 0x96B4f4E4… Ocean.sol): _erc20Wrap / _determineTransferAmount / _convertDecimals (v3 L682-L704,
//! L913-L976), _erc20Unwrap L726-L741, _etherUnwrap L823-L829, _calculateUnwrapFee L987-L989.

use alloy_primitives::{I256, U256};

use crate::core::types::ProteusData;

use super::proteus_abdk::divi;
use super::proteus_curve::{self as curve, Known, Slice, max_m, min_balance, min_m};
use super::ten::pow10;

/// FIXED_FEE = 10^9 (L31).
fn fixed_fee() -> U256 {
    U256::from(1_000_000_000u64)
}

/// INT_MAX = uint256(type(int256).max) (L15).
fn int_max() -> U256 {
    I256::MAX.into_raw()
}

/// _applyFeeByRounding (L788-L817): модуль < 2·FIXED_FEE — откат; fee_up — модуль + модуль/BASE_FEE + FIXED_FEE,
/// иначе модуль − модуль/BASE_FEE − FIXED_FEE; знак сохраняется. None — откат / переполнение.
fn apply_fee(amount: I256, fee_up: bool, base_fee: U256) -> Option<I256> {
    let abs = amount.unsigned_abs();
    if abs < fixed_fee() * U256::from(2u8) || base_fee.is_zero() {
        return None;
    }
    let r = if fee_up {
        let r = abs.checked_add(abs / base_fee)?.checked_add(fixed_fee())?;
        if r >= int_max() {
            return None;
        }
        r
    } else {
        abs.checked_sub(abs / base_fee)?.checked_sub(fixed_fee())?
    };
    let v = I256::try_from(r).ok()?;
    if amount.is_negative() { v.checked_neg() } else { Some(v) }
}

/// _checkAmountWithBalance (L753-L760): баланс / сумма ≥ 10^11 — откат (сумма 0 — деление на ноль). Ошибок нет.
fn amount_ok(balance: U256, amount: U256) -> bool {
    !amount.is_zero() && balance / amount < U256::from(100_000_000_000u64)
}

/// _checkBalances (L768-L778): оба баланса ≥ MIN_BALANCE и MIN_M ≤ y/x < MAX_M. Ошибок нет.
fn balances_ok(x: I256, y: I256) -> bool {
    if x < min_balance() || y < min_balance() {
        return false;
    }
    divi(y, x).is_some_and(|r| r >= min_m() && r < max_m())
}

/// Срезы состояния (a, b, k по номеру). None — длины не сходятся (наклонов на один меньше, чем срезов).
fn slices(d: &ProteusData) -> Option<Vec<Slice>> {
    let n = d.a.len();
    if n == 0 || d.b.len() != n || d.k.len() != n || d.slopes.len() + 1 != n {
        return None;
    }
    Some((0..n).map(|i| Slice { a: d.a[i], b: d.b[i], k: d.k[i] }).collect())
}

/// swapGivenInputAmount реализации (L58-L84 → _swap L264-L332, FEE_DOWN). Принимает: состояние, балансы ДО обмена
/// (единицы учёта), вход (единицы учёта), вход — сторона x. Возвращает: выход (единицы учёта) или None (откат).
pub fn swap_given_input(
    d: &ProteusData,
    xb: U256,
    yb: U256,
    input: U256,
    in_x: bool,
) -> Option<U256> {
    if input >= int_max() || xb >= int_max() || yb >= int_max() {
        return None;
    }
    let (b_in, b_out) = if in_x { (xb, yb) } else { (yb, xb) };
    if !amount_ok(b_in, input) {
        return None;
    }
    let sl = slices(d)?;
    let (xi, yi) = (I256::from_raw(xb), I256::from_raw(yb));
    let spec = I256::from_raw(input);
    let rounded = apply_fee(spec, false, d.base_fee)?;
    let i = curve::find_slice(&d.slopes, xi, yi)?;
    let u = curve::utility(d.utility, xi, yi, *sl.get(i)?)?;
    let neg = rounded.is_negative();
    let (xf, yf) = if in_x {
        let fixed = xi.checked_add(rounded)?;
        curve::final_point(fixed, Known::X, if neg { 1 } else { -1 }, u, i, &d.slopes, &sl)?
    } else {
        let fixed = yi.checked_add(rounded)?;
        curve::final_point(fixed, Known::Y, if neg { -1 } else { 1 }, u, i, &d.slopes, &sl)?
    };
    let computed = if in_x {
        let c = apply_fee(yf.checked_sub(yi)?, false, d.base_fee)?;
        balances_ok(xi.checked_add(spec)?, yi.checked_add(c)?).then_some(c)?
    } else {
        let c = apply_fee(xf.checked_sub(xi)?, false, d.base_fee)?;
        balances_ok(xi.checked_add(c)?, yi.checked_add(spec)?).then_some(c)?
    };
    // require(result < 0) (L78), выход = −result; проверка выхода против баланса другой стороны (L80-L83).
    if !computed.is_negative() {
        return None;
    }
    let out = computed.unsigned_abs();
    amount_ok(b_out, out).then_some(out)
}

/// Сумма токена (знаки dec) → единицы учёта для wrapErc20 точной суммой. None — знаков больше 18 (учёт округлил бы
/// вход вверх с пылью — шаг исполнителя так не строится) или переполнение. Ошибок нет.
pub fn to_ocean(amount: U256, dec: u8) -> Option<U256> {
    if dec > 18 {
        return None;
    }
    amount.checked_mul(pow10(u64::from(18 - dec)))
}

/// unwrap учёта (_erc20Unwrap / _etherUnwrap): комиссия = сумма / делитель, остаток → знаки токена вниз (у нативной
/// стороны знаков 18 — без перевода). None — делитель 0. Ошибок нет.
pub fn from_ocean(amount: U256, dec: u8, divisor: U256) -> Option<U256> {
    if divisor.is_zero() {
        return None;
    }
    let rest = amount - amount / divisor;
    Some(if dec <= 18 {
        rest / pow10(u64::from(18 - dec))
    } else {
        rest.checked_mul(pow10(u64::from(dec - 18)))?
    })
}

/// Полный обмен шага SHELL с точным входом. Принимает: состояние, токен входа — сторона x, вход (знаки токена входа).
/// Возвращает: выход (знаки токена выхода) или None (заморожен, откат реализации, вход не в учёт).
pub fn quote_in(d: &ProteusData, in_x: bool, amount: U256) -> Option<U256> {
    if d.frozen {
        return None;
    }
    let (dec_in, dec_out) = if in_x { (d.dec_x, d.dec_y) } else { (d.dec_y, d.dec_x) };
    let input = to_ocean(amount, dec_in)?;
    let out = swap_given_input(d, d.x_bal, d.y_bal, input, in_x)?;
    from_ocean(out, dec_out, d.unwrap_fee_divisor)
}

/// Наименьший вход (единицы учёта), который реализация принимает при балансе входа: 2·FIXED_FEE и баланс / вход < 10^11.
/// Ошибок нет.
pub fn min_input(balance_in: U256) -> U256 {
    let by_ratio = balance_in / U256::from(100_000_000_000u64) + U256::from(1u8);
    by_ratio.max(fixed_fee() * U256::from(2u8))
}
