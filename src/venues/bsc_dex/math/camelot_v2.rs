//! Файл: venues/bsc_dex/math/camelot_v2.rs
//! Что делает: решение 443 (T433-F4) — математика пары Camelot V2 (Arbitrum One): порт `getAmountOut` /
//! `_getAmountOut` / `_k` / `_get_y` / `_f` / `_d` пары CamelotPair на целых числах U256 с теми же округлениями и той
//! же арифметикой Solidity 0.5.16: SafeMath (`.mul/.add/.sub`) откатывается при переполнении → `None`; голые `*`, `+`,
//! `-` в 0.5 НЕ проверяются — переполнение по модулю 2^256 (`wrapping_*`); деление на ноль откатывается → `None`.
//! Комиссия — НАПРАВЛЕННАЯ: своя у входа token0 и у входа token1 (token0FeePercent / token1FeePercent, над
//! FEE_DENOMINATOR = 100000). Волатильная пара — формула Uniswap V2 с комиссией (FEE_DENOMINATOR − fee)/FEE_DENOMINATOR;
//! стабильная — инвариант `x³y + xy³` (Ньютон до |Δy| ≤ 1 за 255 раундов, без отката при несходимости).
//! Сверх getAmountOut — проверки самого обмена (swap): выход > 0 (L259), выход < резерва выхода (L262) и проверка K
//! (L320-L323): `_k(баланс0 − комиссия0, баланс1 − комиссия1) ≥ _k(резерв0, резерв1)`, где баланс входа = резерв + вход,
//! баланс выхода = резерв − выход, комиссия = floor(вход·fee/FEE_DENOMINATOR) (L280-L281; доли реферера и владельца
//! уходят из баланса и из комиссии одновременно — L286-L318, разность та же). Не выполнена — своп откатился бы, `None`.
//! Кто вызывает: quote/pool_quote.rs (ветка Solidly при виде camelot_v2); тесты (tests/arb_dex_camelot_v2.rs).
//! От чего зависит: math/solidly_math.rs (e18).
//! Настройки: нет (резервы, флаг stable, множители точности, комиссии — с контракта пары, pools/load_camelot.rs).
//!
//! Источник: verified-исходник Arbiscan официальной фабрики Camelot V2 0x6EcCab422D763aC031210895C81787E87B43A652
//! (адрес — docs.camelot.exchange/contracts/arbitrum/one-mainnet, «AMMv2 Factory»); фабрика создаёт пары кодом
//! `type(CamelotPair).creationCode` (CamelotFactory.sol L59) — текст CamelotPair в её исходнике (строки 390-814)
//! построчно совпадает с verified-исходником пары 0x1c31fb3359357f6436565ccb3e982bc6bf4189ae. Официального репозитория
//! Camelot V2 на GitHub нет (у github.com/CamelotLabs его нет); копия verified-исходников — зеркало
//! tintinweb/smart-contract-sanctuary-arbitrum@465b14914e025d2892dd0b484ef8bf8b1efbefe3,
//! contracts/mainnet/1c/1c31fb3359357f6436565ccb3e982bc6bf4189ae_CamelotPair.sol (далее «Pair.sol», номера строк — его):
//! FEE_DENOMINATOR L22, MAX_FEE_PERCENT L23, getReserves L46-L51, _swap L258-L327, _k L329-L338, _get_y L340-L362,
//! _f L364-L366, _d L368-L370, getAmountOut L372-L375, _getAmountOut L377-L394.

use alloy_primitives::U256;

use super::solidly_math::e18;

/// FEE_DENOMINATOR пары (Pair.sol L22: `uint public constant FEE_DENOMINATOR = 100000`) — константа протокола.
pub const FEE_DENOMINATOR: u32 = 100_000;
/// MAX_FEE_PERCENT пары (Pair.sol L23: `= 2000; // = 2%`): setFeePercent больше не даёт (L99) — константа протокола.
pub const MAX_FEE_PERCENT: u32 = 2_000;

/// Состояние пары, нужное расчёту: резервы, флаг stable, множители точности (10^decimals) и комиссии входа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    pub reserve0: U256,
    pub reserve1: U256,
    pub stable: bool,
    /// precisionMultiplier0 = 10^decimals(token0) (Pair.sol L86).
    pub pm0: U256,
    /// precisionMultiplier1 = 10^decimals(token1) (Pair.sol L87).
    pub pm1: U256,
    /// token0FeePercent — комиссия входа token0 (над FEE_DENOMINATOR).
    pub fee0: U256,
    /// token1FeePercent — комиссия входа token1.
    pub fee1: U256,
}

/// Pair._k (L329-L338): stable — `_x = b0·1e18/pm0`, `_y = b1·1e18/pm1`, `_a = _x·_y/1e18`,
/// `_b = _x·_x/1e18 + _y·_y/1e18`, `_a·_b/1e18` (всё SafeMath); volatile — `b0·b1` (SafeMath). Принимает: балансы,
/// флаг, множители. Возвращает: инвариант. None — переполнение SafeMath или деление на ноль (откат).
pub fn k(b0: U256, b1: U256, stable: bool, pm0: U256, pm1: U256) -> Option<U256> {
    if !stable {
        return b0.checked_mul(b1);
    }
    let one = e18();
    let x = b0.checked_mul(one)?.checked_div(pm0)?;
    let y = b1.checked_mul(one)?.checked_div(pm1)?;
    let a = x.checked_mul(y)? / one;
    let b = (x.checked_mul(x)? / one).checked_add(y.checked_mul(y)? / one)?;
    Some(a.checked_mul(b)? / one)
}

/// Pair._f (L364-L366), голая арифметика 0.5: `x0·(y·y/1e18·y/1e18)/1e18 + (x0·x0/1e18·x0/1e18)·y/1e18`. Ошибок нет
/// (переполнение — по модулю 2^256, как в контракте).
pub fn f(x0: U256, y: U256) -> U256 {
    let one = e18();
    let y3 = (y.wrapping_mul(y) / one).wrapping_mul(y) / one;
    let x3 = (x0.wrapping_mul(x0) / one).wrapping_mul(x0) / one;
    (x0.wrapping_mul(y3) / one).wrapping_add(x3.wrapping_mul(y) / one)
}

/// Pair._d (L368-L370), голая арифметика: `3·x0·(y·y/1e18)/1e18 + (x0·x0/1e18·x0/1e18)`. Ошибок нет.
pub fn d(x0: U256, y: U256) -> U256 {
    let one = e18();
    let x3 = (x0.wrapping_mul(x0) / one).wrapping_mul(x0) / one;
    (U256::from(3u8).wrapping_mul(x0).wrapping_mul(y.wrapping_mul(y) / one) / one).wrapping_add(x3)
}

/// Pair._get_y (L340-L362): Ньютон до 255 раундов, шаг `dy = |xy − k|·1e18 / _d`, выход при |y − y_prev| ≤ 1; после
/// 255 раундов — последнее y. Голая арифметика (переполнение по модулю). Принимает: x0, xy, стартовое y. Возвращает:
/// новое y. None — `_d = 0` (деление на ноль — откат).
pub fn get_y(x0: U256, xy: U256, mut y: U256) -> Option<U256> {
    let one = e18();
    for _ in 0..255u16 {
        let y_prev = y;
        let kk = f(x0, y);
        if kk < xy {
            let dy = (xy - kk).wrapping_mul(one).checked_div(d(x0, y))?;
            y = y.wrapping_add(dy);
        } else {
            let dy = (kk - xy).wrapping_mul(one).checked_div(d(x0, y))?;
            y = y.wrapping_sub(dy);
        }
        let step = if y > y_prev { y - y_prev } else { y_prev - y };
        if step <= U256::from(1u8) {
            return Some(y);
        }
    }
    Some(y)
}

/// Pair.getAmountOut (L372-L375) + _getAmountOut (L377-L394): комиссия — входа (token0FeePercent при входе token0,
/// иначе token1FeePercent). Принимает: состояние пары, вход, направление (true — вход token0). Возвращает: выход, как
/// его вернёт пара. None — откат (SafeMath, деление на ноль).
pub fn get_amount_out(p: &Pair, amount_in: U256, zero_for_one: bool) -> Option<U256> {
    let den = U256::from(FEE_DENOMINATOR);
    let fee = if zero_for_one { p.fee0 } else { p.fee1 };
    if p.stable {
        // L379: amountIn = amountIn.sub(amountIn.mul(feePercent) / FEE_DENOMINATOR).
        let amount_in = amount_in.checked_sub(amount_in.checked_mul(fee)? / den)?;
        // L380: xy = _k(_reserve0, _reserve1) — по исходным резервам.
        let xy = k(p.reserve0, p.reserve1, true, p.pm0, p.pm1)?;
        let one = e18();
        // L381-L382: нормализация резервов (голые `*`).
        let r0 = p.reserve0.wrapping_mul(one).checked_div(p.pm0)?;
        let r1 = p.reserve1.wrapping_mul(one).checked_div(p.pm1)?;
        let (ra, rb, pm_in, pm_out) = if zero_for_one {
            (r0, r1, p.pm0, p.pm1)
        } else {
            (r1, r0, p.pm1, p.pm0)
        };
        // L385-L387.
        let a = amount_in.wrapping_mul(one).checked_div(pm_in)?;
        let y = rb.wrapping_sub(get_y(a.wrapping_add(ra), xy, rb)?);
        Some(y.wrapping_mul(pm_out) / one)
    } else {
        let (ra, rb) = if zero_for_one {
            (p.reserve0, p.reserve1)
        } else {
            (p.reserve1, p.reserve0)
        };
        // L391-L392 (SafeMath).
        let a = amount_in.checked_mul(den.checked_sub(fee)?)?;
        let num = a.checked_mul(rb)?;
        num.checked_div(ra.checked_mul(den)?.checked_add(a)?)
    }
}

/// Выход обмена на точный вход, как его исполнит пара: getAmountOut и проверки swap — выход > 0 (L259), выход меньше
/// резерва выхода (L262) и K (L320-L323, k_holds). Принимает: состояние, вход, направление. Возвращает: выход. None —
/// обмен откатился бы.
pub fn swap_out(p: &Pair, amount_in: U256, zero_for_one: bool) -> Option<U256> {
    let out = get_amount_out(p, amount_in, zero_for_one)?;
    let r_out = if zero_for_one {
        p.reserve1
    } else {
        p.reserve0
    };
    if out.is_zero() || out >= r_out || !k_holds(p, amount_in, out, zero_for_one)? {
        return None;
    }
    Some(out)
}

/// Проверка K обмена (Pair.sol L276-L281, L320-L323): балансы после обмена минус комиссия входа (SafeMath) — инвариант не
/// меньше, чем у резервов. Принимает: состояние, вход, выход, направление. Возвращает: выполнена ли. None — откат
/// SafeMath (переполнение, недобор, деление на ноль в _k).
pub fn k_holds(p: &Pair, amount_in: U256, out: U256, zero_for_one: bool) -> Option<bool> {
    let den = U256::from(FEE_DENOMINATOR);
    let (fee, r_in, r_out) = if zero_for_one {
        (p.fee0, p.reserve0, p.reserve1)
    } else {
        (p.fee1, p.reserve1, p.reserve0)
    };
    let cut = amount_in.checked_mul(fee)? / den;
    let b_in = r_in.checked_add(amount_in)?.checked_sub(cut)?;
    let b_out = r_out.checked_sub(out)?;
    let (b0, b1) = if zero_for_one {
        (b_in, b_out)
    } else {
        (b_out, b_in)
    };
    let after = k(b0, b1, p.stable, p.pm0, p.pm1)?;
    Some(after >= k(p.reserve0, p.reserve1, p.stable, p.pm0, p.pm1)?)
}
