//! Файл: venues/bsc_dex/math/stableswap_exchange.rs
//! Что делает: выход обмена пула StableSwap (`exchange`) по шаблону (math/stableswap_v1.rs: Template): dx монеты i →
//! монета j — `x = xp[i] + dx·rates[i]/1e18`, `dy = xp[j] − y − 1`, комиссия `dy·fee/1e10` до перевода в единицы
//! монеты, выход `(dy − комиссия)·1e18/rates[j]` (то, что пул переводит). Вынесено из stableswap_v1.rs (ориентир 200
//! строк); переэкспорт — там же.
//! Кто вызывает: math/stableswap_v1.rs (переэкспорт), math/stable_guard.rs, quote/*; тесты.
//! От чего зависит: math/stableswap_v1.rs, alloy-primitives.
//! Настройки: нет.
//!
//! Источники: ellipsis-finance/ellipsis 6bdf778 StableSwap.vy `exchange` L434-L470; pancake-smart-contracts cb079908
//! PancakeStableSwapTwoPool.sol `exchange` L425-L466; lista-dao/moolah eb7d73a StableSwapPool.sol L478-L520;
//! nerve-finance/contracts bda7578 SwapUtils.sol `_calculateSwap` L611-L630.

use alloy_primitives::U256;

use super::stableswap_v1::{Template, curve2020, fee_denominator, get_y_t, precision, xp};

/// `exchange` шаблона Curve 2020 (см. exchange_out_t). Ошибок нет: None — как у exchange_out_t.
pub fn exchange_out(
    i: usize,
    j: usize,
    dx: U256,
    balances: &[U256],
    rates: &[U256],
    amp: U256,
    fee: U256,
) -> Option<U256> {
    exchange_parts(i, j, dx, balances, rates, amp, fee, &curve2020()).map(|p| p.0)
}

/// `exchange` (выход, который пул переводит): dx монеты i → монета j. Принимает: i, j, dx, балансы, rates, A,
/// fee (над 10^10), шаблон. Возвращает: выход в единицах монеты j. None — как у get_y_t, недобор `xp[j] − y − 1`.
#[allow(clippy::too_many_arguments)]
pub fn exchange_out_t(
    i: usize,
    j: usize,
    dx: U256,
    balances: &[U256],
    rates: &[U256],
    amp: U256,
    fee: U256,
    t: &Template,
) -> Option<U256> {
    exchange_parts(i, j, dx, balances, rates, amp, fee, t).map(|p| p.0)
}

/// `exchange` с комиссией в 18 знаках: (выход в единицах монеты j, dy_fee в 18 знаках). Нужна проверке цены после
/// обмена (math/stable_guard.rs: новый баланс j = старый − выход − доля админа). None — как у exchange_out_t.
#[allow(clippy::too_many_arguments)]
pub fn exchange_parts(
    i: usize,
    j: usize,
    dx: U256,
    balances: &[U256],
    rates: &[U256],
    amp: U256,
    fee: U256,
    t: &Template,
) -> Option<(U256, U256)> {
    let xp = xp(balances, rates)?;
    let p = precision();
    let x = xp
        .get(i)?
        .checked_add(dx.checked_mul(*rates.get(i)?)? / p)?;
    let y = get_y_t(i, j, x, &xp, amp, t)?;
    let dy = xp.get(j)?.checked_sub(y)?.checked_sub(U256::from(1u8))?;
    let dy_fee = dy.checked_mul(fee)? / fee_denominator();
    let rj = *rates.get(j)?;
    if rj.is_zero() {
        return None;
    }
    Some((dy.checked_sub(dy_fee)?.checked_mul(p)? / rj, dy_fee))
}
