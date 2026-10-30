//! Файл: venues/bsc_dex/math/curve_math.rs
//! Что делает: базовая математика инварианта Curve StableSwap (решение 80 — экзотическая математика Curve/Ellipsis
//! stableswap «по вкладу ликвидности») — порт официального контракта на целых числах U256, с теми же округлениями и
//! той же сходимостью Ньютона, что в Vyper-исходнике. Две функции: `get_d` (инвариант D итерацией Ньютона по
//! A·n^n·Σx + D = A·D·n^n + D^(n+1)/(n^n·Πx)) и `get_y` (новый баланс монеты j при заданном балансе монеты i, решение
//! квадратного уравнения итерацией). Это чистая математика без сети; уровень свопа с комиссией (`get_dy`) — в
//! math/swap_curve.rs. Ellipsis на BSC — форк Curve с тем же инвариантом; берём официальный публичный Curve.
//! Охват фазы A: PLAIN StableSwap (не meta, не crypto/tricrypto, не lending) с РАВНЫМИ rate-множителями (RATES[k] =
//! PRECISION, т.е. все монеты 18 знаков). `amp` — это A С ПРЕФАКТОРОМ A_PRECISION (как `self._A()` контракта, т.е.
//! A·100). Разные decimals/rates и meta-пулы — фаза B (там rates читаются с контракта и передаются как xp).
//! Кто вызывает: math/swap_curve.rs (get_dy); тесты (tests/bsc_dex_curve_math.rs) — через переэкспорт в swap_curve.
//! От чего зависит: alloy-primitives (U256). Настройки: нет (balances, A, fee — с контракта пула в фазе B).
//!
//! Источники (официальные, решение 23): curvefi/curve-contract (HEAD 574f44027d089de0eac765f5a74ea5ae96aba968),
//! contracts/pool-templates/base/SwapTemplateBase.vy (@version ^0.2.8): `_get_D`, `_get_y`. Vyper 0.2.x проверяет
//! всю арифметику (+,-,* откатываются при переполнении/недоборе) → порт целиком на checked_*; где контракт откатился
//! бы (переполнение, вычитание ниже нуля, деление на ноль, `raise` при несходимости за 255 раундов) — `None`.
//! НЕ rust_decimal: целочисленно, ровно как uint256 контракта.

use alloy_primitives::U256;

/// Префактор точности A (контракт: A_PRECISION = 100). `amp` в этих функциях уже умножен на него. Ошибок нет.
pub fn a_precision() -> U256 {
    U256::from(100u8)
}

/// Единица перевода цены/точности (контракт: PRECISION = 10^18). Ошибок нет.
pub fn precision() -> U256 {
    U256::from(10u8).pow(U256::from(18u8))
}

/// Знаменатель комиссии пула (контракт: FEE_DENOMINATOR = 10^10). Ошибок нет.
pub fn fee_denominator() -> U256 {
    U256::from(10u8).pow(U256::from(10u8))
}

/// SwapTemplateBase._get_D: инвариант D по балансам `xp` (в единицах PRECISION) и `amp` (= A·A_PRECISION).
/// Итерация Ньютона до точности 1 (как в контракте). Принимает: xp — N балансов, amp — A с префактором.
/// Возвращает: D. None — Σxp = 0 даёт D = 0 (это Some(0)); None именно при переполнении/недоборе любой операции
/// SafeMath, делении на ноль (нулевой баланс монеты → `_x·N = 0`) или несходимости за 255 раундов (контракт: `raise`).
pub fn get_d(xp: &[U256], amp: U256) -> Option<U256> {
    let n = U256::from(xp.len());
    let one = U256::from(1u8);
    let a_prec = a_precision();

    let mut s = U256::ZERO;
    for &x in xp {
        s = s.checked_add(x)?;
    }
    if s.is_zero() {
        return Some(U256::ZERO);
    }

    let mut d = s;
    let ann = amp.checked_mul(n)?;
    for _ in 0..255u16 {
        // D_P = D^(n+1) / (n^n · Πxp), накапливается по монетам: D_P = D_P · D / (x · N).
        let mut d_p = d;
        for &x in xp {
            let denom = x.checked_mul(n)?;
            if denom.is_zero() {
                return None; // контракт: деление на ноль при нулевом балансе
            }
            d_p = d_p.checked_mul(d)? / denom;
        }
        let d_prev = d;
        // D = (Ann·S/A_PRECISION + D_P·N) · D / ((Ann − A_PRECISION)·D/A_PRECISION + (N+1)·D_P).
        let numerator = (ann.checked_mul(s)? / a_prec)
            .checked_add(d_p.checked_mul(n)?)?
            .checked_mul(d)?;
        let denom = (ann.checked_sub(a_prec)?.checked_mul(d)? / a_prec)
            .checked_add(n.checked_add(one)?.checked_mul(d_p)?)?;
        if denom.is_zero() {
            return None;
        }
        d = numerator / denom;
        // Равенство с точностью до 1 (как в контракте).
        if d > d_prev {
            if d - d_prev <= one {
                return Some(d);
            }
        } else if d_prev - d <= one {
            return Some(d);
        }
    }
    None // контракт: `raise` — не сошлось за 255 раундов
}

/// SwapTemplateBase._get_y: новый баланс монеты j, если баланс монеты i стал равен `x` (все в единицах PRECISION).
/// Решение квадратного уравнения итерацией (как в контракте): x_1 = (x_1² + c) / (2·x_1 + b − D). D считается внутри
/// по `amp` (ровно как `_get_y` контракта зовёт `_get_D`). Принимает: i, j — индексы монет; x — новый баланс i;
/// xp — текущие балансы; amp — A·A_PRECISION. Возвращает: баланс j. None — неверные индексы (i==j или вне диапазона),
/// переполнение/недобор/деление на ноль или несходимость за 255 раундов (контракт: `raise`).
pub fn get_y(i: usize, j: usize, x: U256, xp: &[U256], amp: U256) -> Option<U256> {
    let n_usize = xp.len();
    if i == j || i >= n_usize || j >= n_usize {
        return None; // контракт: assert i != j / j<N / i<N
    }
    let n = U256::from(n_usize);
    let one = U256::from(1u8);
    let two = U256::from(2u8);
    let a_prec = a_precision();

    let d = get_d(xp, amp)?;
    let ann = amp.checked_mul(n)?;
    if ann.is_zero() {
        return None; // деление на Ann ниже откатилось бы
    }

    let mut c = d;
    let mut s = U256::ZERO;
    for (idx, &xp_idx) in xp.iter().enumerate() {
        let x_i = if idx == i {
            x
        } else if idx != j {
            xp_idx
        } else {
            continue;
        };
        s = s.checked_add(x_i)?;
        let denom = x_i.checked_mul(n)?;
        if denom.is_zero() {
            return None;
        }
        c = c.checked_mul(d)? / denom;
    }
    // c = c · D · A_PRECISION / (Ann · N).
    let c_denom = ann.checked_mul(n)?;
    if c_denom.is_zero() {
        return None;
    }
    c = c.checked_mul(d)?.checked_mul(a_prec)? / c_denom;
    // b = S + D · A_PRECISION / Ann  (член −D учтён в знаменателе итерации).
    let b = s.checked_add(d.checked_mul(a_prec)? / ann)?;

    let mut y = d;
    for _ in 0..255u16 {
        let y_prev = y;
        // y = (y² + c) / (2·y + b − D). Знаменатель — checked_sub(D): недобор → None.
        let numerator = y.checked_mul(y)?.checked_add(c)?;
        let denom = two.checked_mul(y)?.checked_add(b)?.checked_sub(d)?;
        if denom.is_zero() {
            return None;
        }
        y = numerator / denom;
        if y > y_prev {
            if y - y_prev <= one {
                return Some(y);
            }
        } else if y_prev - y <= one {
            return Some(y);
        }
    }
    None // контракт: `raise`
}
