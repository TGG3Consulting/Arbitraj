//! Файл: venues/bsc_dex/math/balancer_linear.rs
//! Что делает: решение 473 (T450-06) — порт LinearMath линейного пула Balancer V2: номинальный баланс главного токена
//! (комиссия за выход за цели), обмены главный ↔ обёрнутый и главный / обёрнутый ↔ BPT по виртуальному выпуску BPT.
//! Все суммы и балансы — уже × множители (18 знаков); масштабирование — math/balancer_v2_more.rs.
//! Кто вызывает: math/balancer_v2_more.rs.
//! От чего зависит: math/balancer_math.rs (FixedPoint mulDown / divDown, ONE), alloy-primitives.
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8, build-info задач 20211208-aave-linear-pool …
//! 20230410-aave-linear-pool-v5 — LinearMath.sol у всех девяти задач Arbitrum один текст (отличие — только порядок
//! двух import); строки — v2/deprecated/20230410-aave-linear-pool-v5
//! (@balancer-labs/v2-pool-linear/contracts/LinearMath.sol):
//! Params L49-L53, _calcBptOutPerMainIn L55-L76, _calcBptInPerMainOut L78-L92, _calcWrappedOutPerMainIn L94-L104,
//! _calcWrappedInPerMainOut L106-L116, _calcMainInPerBptOut L118-L140, _calcMainOutPerBptIn L142-L157,
//! _calcMainOutPerWrappedIn L159-L170, _calcMainInPerWrappedOut L172-L183, _calcBptOutPerWrappedIn L185-L210,
//! _calcBptInPerWrappedOut L212-L230, _calcWrappedInPerBptOut L232-L255, _calcWrappedOutPerBptIn L257-L273,
//! _calcInvariant L275-L277, _toNominal L279-L292, _fromNominal L294-L304. FixedPoint.add / sub и Math.mul / divDown /
//! divUp — с откатом (здесь None).

use alloy_primitives::U256;

use super::balancer_math::{div_down, mul_down, one};

/// Параметры пула: комиссия, нижняя и верхняя цель главного токена (18 знаков).
#[derive(Clone, Copy)]
pub struct Params {
    pub fee: U256,
    pub lower: U256,
    pub upper: U256,
}

/// Балансы пула (18 знаков): главный, обёрнутый, виртуальный выпуск BPT.
#[derive(Clone, Copy)]
pub struct Bal {
    pub main: U256,
    pub wrapped: U256,
    pub supply: U256,
}

/// Math.divDown / divUp. None — деление на ноль.
fn div(a: U256, b: U256, up: bool) -> Option<U256> {
    if b.is_zero() {
        return None;
    }
    if up && !a.is_zero() {
        Some((a - U256::from(1u8)) / b + U256::from(1u8))
    } else {
        Some(a / b)
    }
}

/// _toNominal (L279-L292). None — откат sub.
pub fn to_nominal(real: U256, p: &Params) -> Option<U256> {
    if real < p.lower {
        real.checked_sub(mul_down(p.lower - real, p.fee)?)
    } else if real <= p.upper {
        Some(real)
    } else {
        real.checked_sub(mul_down(real - p.upper, p.fee)?)
    }
}

/// _fromNominal (L294-L304). None — откат.
pub fn from_nominal(nominal: U256, p: &Params) -> Option<U256> {
    if nominal < p.lower {
        let num = nominal.checked_add(mul_down(p.fee, p.lower)?)?;
        div_down(num, one().checked_add(p.fee)?)
    } else if nominal <= p.upper {
        Some(nominal)
    } else {
        let num = nominal.checked_sub(mul_down(p.fee, p.upper)?)?;
        div_down(num, one().checked_sub(p.fee)?)
    }
}

/// Какой токен: главный, обёрнутый, BPT.
#[derive(Clone, Copy, PartialEq)]
pub enum Tok {
    Main,
    Wrapped,
    Bpt,
}

/// Обмен на точный вход (LinearPool._onSwapGivenIn: _swapGivenBptIn / MainIn / WrappedIn). Принимает: токен входа и
/// выхода, вход, балансы, параметры. Возвращает: выход (18 знаков). None — откат пула (тот же токен, арифметика).
pub fn out_given_in(t_in: Tok, t_out: Tok, a: U256, b: &Bal, p: &Params) -> Option<U256> {
    let nom = to_nominal(b.main, p)?;
    let inv = nom.checked_add(b.wrapped)?;
    match (t_in, t_out) {
        // _calcMainOutPerBptIn L142-L157.
        (Tok::Bpt, Tok::Main) => {
            let delta = div(inv.checked_mul(a)?, b.supply, false)?;
            b.main.checked_sub(from_nominal(nom.checked_sub(delta)?, p)?)
        }
        // _calcWrappedOutPerBptIn L257-L273.
        (Tok::Bpt, Tok::Wrapped) => {
            let nb = b.supply.checked_sub(a)?;
            let nw = div(nb.checked_mul(inv)?, b.supply, true)?.checked_sub(nom)?;
            b.wrapped.checked_sub(nw)
        }
        // _calcBptOutPerMainIn L55-L76.
        (Tok::Main, Tok::Bpt) => {
            if b.supply.is_zero() {
                return to_nominal(a, p);
            }
            let after = to_nominal(b.main.checked_add(a)?, p)?;
            div(b.supply.checked_mul(after.checked_sub(nom)?)?, inv, false)
        }
        // _calcWrappedOutPerMainIn L94-L104.
        (Tok::Main, Tok::Wrapped) => to_nominal(b.main.checked_add(a)?, p)?.checked_sub(nom),
        // _calcBptOutPerWrappedIn L185-L210.
        (Tok::Wrapped, Tok::Bpt) => {
            if b.supply.is_zero() {
                return Some(a);
            }
            let ni = nom.checked_add(b.wrapped.checked_add(a)?)?;
            div(b.supply.checked_mul(ni)?, inv, false)?.checked_sub(b.supply)
        }
        // _calcMainOutPerWrappedIn L159-L170.
        (Tok::Wrapped, Tok::Main) => {
            b.main.checked_sub(from_nominal(nom.checked_sub(a)?, p)?)
        }
        _ => None,
    }
}

/// Обмен на точный выход (LinearPool._onSwapGivenOut: _swapGivenBptOut / MainOut / WrappedOut). Принимает: токен
/// входа и выхода, выход, балансы, параметры. Возвращает: вход (18 знаков). None — откат пула.
pub fn in_given_out(t_in: Tok, t_out: Tok, a: U256, b: &Bal, p: &Params) -> Option<U256> {
    let nom = to_nominal(b.main, p)?;
    let inv = nom.checked_add(b.wrapped)?;
    match (t_in, t_out) {
        // _calcMainInPerBptOut L118-L140.
        (Tok::Main, Tok::Bpt) => {
            if b.supply.is_zero() {
                return from_nominal(a, p);
            }
            let delta = div(inv.checked_mul(a)?, b.supply, true)?;
            from_nominal(nom.checked_add(delta)?, p)?.checked_sub(b.main)
        }
        // _calcWrappedInPerBptOut L232-L255.
        (Tok::Wrapped, Tok::Bpt) => {
            if b.supply.is_zero() {
                return Some(a);
            }
            let nb = b.supply.checked_add(a)?;
            let nw = div(nb.checked_mul(inv)?, b.supply, true)?.checked_sub(nom)?;
            nw.checked_sub(b.wrapped)
        }
        // _calcBptInPerMainOut L78-L92.
        (Tok::Bpt, Tok::Main) => {
            let after = to_nominal(b.main.checked_sub(a)?, p)?;
            div(b.supply.checked_mul(nom.checked_sub(after)?)?, inv, true)
        }
        // _calcWrappedInPerMainOut L106-L116.
        (Tok::Wrapped, Tok::Main) => nom.checked_sub(to_nominal(b.main.checked_sub(a)?, p)?),
        // _calcBptInPerWrappedOut L212-L230.
        (Tok::Bpt, Tok::Wrapped) => {
            let ni = nom.checked_add(b.wrapped.checked_sub(a)?)?;
            b.supply.checked_sub(div(b.supply.checked_mul(ni)?, inv, false)?)
        }
        // _calcMainInPerWrappedOut L172-L183.
        (Tok::Main, Tok::Wrapped) => {
            from_nominal(nom.checked_add(a)?, p)?.checked_sub(b.main)
        }
        _ => None,
    }
}
