//! Файл: venues/bsc_dex/math/balancer_stable.rs
//! Что делает: порт StableMath стейблсвап-пула Balancer V2 (решение 289): инвариант D итерацией (_calculateInvariant),
//! баланс токена по D и остальным балансам (_getTokenBalanceGivenInvariantAndAllOtherBalances), выход на точный вход и
//! вход на точный выход. Балансы и суммы — уже ×множитель точности (18 знаков), комиссия снята снаружи
//! (math/balancer_math.rs).
//! Кто вызывает: math/balancer_math.rs.
//! От чего зависит: alloy-primitives.
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 v2/deprecated/20210624-stable-pool/build-info/
//! StablePoolFactory.json → contracts/StableMath.sol (= Sourcify пулов ACSI 0xa22f3e51, 0xe901c4e9, exact_match):
//! _AMP_PRECISION = 1e3 (L31), _calculateInvariant L49-L105 (255 раундов, несходимость — откат L104
//! Errors.STABLE_GET_BALANCE_DIDNT_CONVERGE = BAL#322), _calcOutGivenIn L109-L147 (D с округлением вверх, выход − 1),
//! _calcInGivenOut L152-L190 (вход + 1), _getTokenBalanceGivenInvariantAndAllOtherBalances L465-L515 (255 раундов,
//! несходимость — откат L514, тот же BAL#322). Пул ACSI 0xe901c4e9: все записанные запросы хранилища откатываются BAL#322;
//! наш invariant() на записанных балансах блока 133699740 не сходится уже на инварианте (тест
//! tests/bsc_dex_balancer_acsi_live.rs).
//! Решение 450: библиотеки v2 (balancer-deployments@b55dc2b8 v2/deprecated/20220609-stable-pool-v2/build-info/
//! StablePoolFactory.json → contracts/StableMath.sol): _AMP_PRECISION = 1e3 (L30), _calculateInvariant L57-L120 — другая
//! формула (Curve, округление вниз) — invariant_v2; _calcOutGivenIn / _calcInGivenOut / баланс по D — тот же текст.
//! Math.mul / Math.divUp / Math.divDown / FixedPoint.add / sub — с откатом (здесь None); умножения `*` без Math (Solidity
//! 0.7 без проверки) — здесь с проверкой: в пределах пула переполнения нет, переполнение — None.

use alloy_primitives::U256;

/// _AMP_PRECISION (StableMath.sol L31).
const AMP_PRECISION: u64 = 1_000;
/// Предел раундов итерации (L78, L497).
const ROUNDS: usize = 255;

/// Math.divUp / divDown по флагу. None — деление на ноль.
fn div(a: U256, b: U256, up: bool) -> Option<U256> {
    if b.is_zero() {
        return None;
    }
    if up && !a.is_zero() {
        Some(U256::from(1u8) + (a - U256::from(1u8)) / b)
    } else {
        Some(a / b)
    }
}

/// Разница двух чисел не больше 1 — условие схождения (L95-L101, L505-L511).
fn close(a: U256, b: U256) -> bool {
    let d = if a > b { a - b } else { b - a };
    d <= U256::from(1u8)
}

/// _calculateInvariant (L49-L105). Принимает: A × точность, балансы, округление вверх. None — несходимость за 255
/// раундов или откат арифметики.
pub fn invariant(amp: U256, b: &[U256], up: bool) -> Option<U256> {
    let n = U256::from(b.len());
    let mut sum = U256::ZERO;
    for x in b {
        sum = sum.checked_add(*x)?;
    }
    if sum.is_zero() {
        return Some(sum);
    }
    let prec = U256::from(AMP_PRECISION);
    let at = amp.checked_mul(n)?;
    let mut d = sum;
    for _ in 0..ROUNDS {
        let mut p = b.first()?.checked_mul(n)?;
        for x in &b[1..] {
            p = div(p.checked_mul(*x)?.checked_mul(n)?, d, up)?;
        }
        let prev = d;
        let num = n.checked_mul(d)?.checked_mul(d)?.checked_add(div(
            at.checked_mul(sum)?.checked_mul(p)?,
            prec,
            up,
        )?)?;
        let den = (n + U256::from(1u8)).checked_mul(d)?.checked_add(div(
            at.checked_sub(prec)?.checked_mul(p)?,
            prec,
            !up,
        )?)?;
        d = div(num, den, up)?;
        if close(d, prev) {
            return Some(d);
        }
    }
    None
}

/// Решение 450: _calculateInvariant библиотек v2 (20220609-stable-pool-v2 StableMath.sol L57-L120): D_P = D, затем
/// D_P = D_P·D / (b_j·n) по всем монетам; D = ((A·n·S / AMP_PRECISION + D_P·n)·D) / ((A·n − AMP_PRECISION)·D /
/// AMP_PRECISION + (n + 1)·D_P), всё с округлением вниз; 255 раундов, несходимость — откат L119. None — откат.
pub fn invariant_v2(amp: U256, b: &[U256]) -> Option<U256> {
    let n = U256::from(b.len());
    let mut sum = U256::ZERO;
    for x in b {
        sum = sum.checked_add(*x)?;
    }
    if sum.is_zero() {
        return Some(sum);
    }
    let prec = U256::from(AMP_PRECISION);
    let at = amp.checked_mul(n)?;
    let mut d = sum;
    for _ in 0..ROUNDS {
        let mut dp = d;
        for x in b {
            dp = div(dp.checked_mul(d)?, x.checked_mul(n)?, false)?;
        }
        let prev = d;
        let num = div(at.checked_mul(sum)?, prec, false)?
            .checked_add(dp.checked_mul(n)?)?
            .checked_mul(d)?;
        let den = div(at.checked_sub(prec)?.checked_mul(d)?, prec, false)?
            .checked_add((n + U256::from(1u8)).checked_mul(dp)?)?;
        d = div(num, den, false)?;
        if close(d, prev) {
            return Some(d);
        }
    }
    None
}

/// _getTokenBalanceGivenInvariantAndAllOtherBalances (L465-L515). Принимает: A × точность, балансы, D, индекс токена.
/// None — несходимость за 255 раундов или откат арифметики.
fn balance_given(amp: U256, b: &[U256], d: U256, t: usize) -> Option<U256> {
    let n = U256::from(b.len());
    let at = amp.checked_mul(n)?;
    let mut sum = *b.first()?;
    let mut p = b.first()?.checked_mul(n)?;
    for x in &b[1..] {
        p = div(p.checked_mul(*x)?.checked_mul(n)?, d, false)?;
        sum = sum.checked_add(*x)?;
    }
    sum = sum.checked_sub(*b.get(t)?)?;
    let prec = U256::from(AMP_PRECISION);
    let inv2 = d.checked_mul(d)?;
    let c = div(inv2, at.checked_mul(p)?, true)?
        .checked_mul(prec)?
        .checked_mul(b[t])?;
    let bb = sum.checked_add(div(d, at, false)?.checked_mul(prec)?)?;
    let mut y = div(inv2.checked_add(c)?, d.checked_add(bb)?, true)?;
    for _ in 0..ROUNDS {
        let prev = y;
        let den = y
            .checked_mul(U256::from(2u8))?
            .checked_add(bb)?
            .checked_sub(d)?;
        y = div(y.checked_mul(y)?.checked_add(c)?, den, true)?;
        if close(y, prev) {
            return Some(y);
        }
    }
    None
}

/// _calcOutGivenIn (L109-L147). Принимает: A × точность, балансы (18 знаков), индексы, вход без комиссии (18 знаков),
/// версию библиотек (решение 450: v2 — D по invariant_v2, StablePool.sol v2 L145-L164 считает D до вызова; формула
/// выхода и баланс по D — тот же текст, StableMath.sol v2 L125-L164, L474). Возвращает: выход (18 знаков). None — откат
/// пула.
pub fn out_given_in(
    amp: U256,
    b: &[U256],
    i: usize,
    j: usize,
    a_in: U256,
    v2: bool,
) -> Option<U256> {
    let d = if v2 {
        invariant_v2(amp, b)?
    } else {
        invariant(amp, b, true)?
    };
    let mut nb = b.to_vec();
    nb[i] = nb.get(i)?.checked_add(a_in)?;
    let fin = balance_given(amp, &nb, d, j)?;
    b.get(j)?.checked_sub(fin)?.checked_sub(U256::from(1u8))
}

/// _calcInGivenOut (L152-L190). Принимает: A × точность, балансы, индексы, выход (18 знаков), версию библиотек (как у
/// out_given_in). Возвращает: вход без комиссии (18 знаков). None — откат пула.
pub fn in_given_out(
    amp: U256,
    b: &[U256],
    i: usize,
    j: usize,
    a_out: U256,
    v2: bool,
) -> Option<U256> {
    let d = if v2 {
        invariant_v2(amp, b)?
    } else {
        invariant(amp, b, true)?
    };
    let mut nb = b.to_vec();
    nb[j] = nb.get(j)?.checked_sub(a_out)?;
    let fin = balance_given(amp, &nb, d, i)?;
    fin.checked_sub(*b.get(i)?)?.checked_add(U256::from(1u8))
}
