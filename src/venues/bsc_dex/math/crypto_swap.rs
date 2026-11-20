//! Файл: venues/bsc_dex/math/crypto_swap.rs
//! Что делает: выход обмена crypto-пула Curve (решения 442, 453) так, как его считает `_exchange` пула: баланс входа
//! +dx, балансы в 18 знаках и ценах пула (xp); в рампе A/gamma — сначала D = newton_D по балансам ДО обмена (как пул:
//! x0 — старый баланс входа); y по формуле версии пула (CryptoData.math), dy = xp[j] − y − 1, обратно в единицы монеты,
//! комиссия `_fee(xp)` (mid_fee ↔ out_fee по удалённости от равновесия) с выхода. Версии: Twocrypto-NG v2.1.0 / v2.0.0
//! (math/crypto_y2.rs get_y, crypto_d2.rs newton_D), Tricrypto-NG (crypto_y3.rs, crypto_d3.rs), crypto v1 — фабрика
//! (crypto_v1_2.rs) и tricrypto 2021 (crypto_v1_3.rs): у v1 тот же порядок действий обмена (CurveCryptoSwap2ETH.vy
//! `_exchange` L643-L705, CryptoSwap.vy `exchange` L593-L659), все операции проверяемые — значения те же, что у
//! формул NG ниже. Где контракт откатился бы — None.
//! Кто вызывает: quote/pool_quote.rs (ветка Crypto), quote/crypto_depth.rs, quote/spot.rs (spot), quote/steps.rs
//! (fee, scaled).
//! От чего зависит: core/types (CryptoData, CryptoMath), math/crypto_ops.rs, crypto_y2.rs, crypto_y3.rs, crypto_d2.rs,
//! crypto_d3.rs, crypto_v1_2.rs, crypto_v1_3.rs, math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/twocrypto-ng 89c8dfb, contracts/main/CurveTwocryptoOptimized.vy — `exchange` L364-L404 (вход
//! `_transfer_in` L286-L335 уже прибавлен к balances), `_exchange` L745-L810 (рампа L770-L781), `_fee` L1163-L1177;
//! curvefi/tricrypto-ng ecaa816, contracts/main/CurveTricryptoOptimizedWETH.vy — `_exchange` L861-L961 (рампа
//! L901-L913), `_fee` L1289-L1297 (у CurveTricryptoOptimized.vy — те же строки L808-L881, L1282-L1290),
//! CurveCryptoMathOptimized3.vy `_reduction_coefficient` L644-L663; v1 — CurveCryptoSwap2ETH.vy `_fee` L228-L241,
//! CurveCryptoMath3.vy `reduction_coefficient` L74-L91 (те же значения, проверяемые операции).

use alloy_primitives::U256;

use crate::core::types::{CryptoData, CryptoMath};

use super::crypto_ops::{au, du, mu, su, udiv_u};
use super::ten::{E10, E18};
use super::{crypto_d2, crypto_d3, crypto_v1_2, crypto_v1_3, crypto_y2, crypto_y3};

/// Что делает: выход обмена dx монеты i на монету j (точный вход), как `_exchange` пула. Принимает: состояние пула,
/// индексы монет пула, вход в единицах монеты i. Возвращает: dy в единицах монеты j или None (откат: i = j, dx = 0,
/// индекс вне пула, assert математики, переполнение, обмены пула остановлены).
pub fn exchange_out(p: &CryptoData, i: usize, j: usize, dx: U256) -> Option<U256> {
    let n = p.balances.len();
    let shape_ok = n == coins(p.math) && p.precisions.len() == n && p.price_scale.len() == n - 1;
    if p.halted || !shape_ok || i == j || i >= n || j >= n || dx.is_zero() {
        return None;
    }
    // Рампа: D = newton_D(A, gamma, xp с x0 — балансом входа ДО обмена, K0_prev = 0) — это xp старых балансов.
    let d = if p.ramp {
        newton_d(p, &scaled(p, &p.balances)?)?
    } else {
        p.d
    };
    let mut bal = p.balances.clone();
    bal[i] = au(bal[i], dx)?;
    let mut xp = scaled(p, &bal)?;
    let y_out = get_y(p, &xp, d, j)?;
    let mut dy = su(xp[j], y_out)?;
    xp[j] = su(xp[j], dy)?;
    dy = su(dy, U256::from(1u8))?;
    if j > 0 {
        dy = du(mu(dy, E18)?, p.price_scale[j - 1])?;
    }
    dy = du(dy, p.precisions[j])?;
    let fee = udiv_u(mu(fee(p, &xp)?, dy)?, E10);
    dy = su(dy, fee)?;
    // y -= dy: баланс выхода не уходит ниже нуля (проверяемое вычитание пула).
    su(p.balances[j], dy)?;
    Some(dy)
}

/// Число монет версии: Twocrypto / v1 фабрики — 2, Tricrypto / tricrypto v1 — 3. Ошибок нет.
pub fn coins(m: CryptoMath) -> usize {
    match m {
        CryptoMath::TwoNg210 | CryptoMath::TwoNg200 | CryptoMath::TwoV1 => 2,
        CryptoMath::TriNg | CryptoMath::TriV1 => 3,
    }
}

/// y при балансах xp и инварианте d по формуле версии (get_y NG — первое значение, newton_y v1). None — откат.
fn get_y(p: &CryptoData, xp: &[U256], d: U256, j: usize) -> Option<U256> {
    match p.math {
        CryptoMath::TwoNg210 | CryptoMath::TwoNg200 => {
            Some(crypto_y2::get_y(p.math, p.a, p.gamma, [xp[0], xp[1]], d, j)?[0])
        }
        CryptoMath::TriNg => Some(crypto_y3::get_y(p.a, p.gamma, [xp[0], xp[1], xp[2]], d, j)?[0]),
        CryptoMath::TwoV1 => crypto_v1_2::newton_y(p.a, p.gamma, [xp[0], xp[1]], d, j),
        CryptoMath::TriV1 => crypto_v1_3::newton_y(p.a, p.gamma, [xp[0], xp[1], xp[2]], d, j),
    }
}

/// newton_D(A, gamma, xp) по формуле версии (K0_prev = 0). None — откат.
fn newton_d(p: &CryptoData, xp: &[U256]) -> Option<U256> {
    match p.math {
        CryptoMath::TwoNg210 | CryptoMath::TwoNg200 => {
            crypto_d2::newton_d(p.math, p.a, p.gamma, [xp[0], xp[1]])
        }
        CryptoMath::TriNg => crypto_d3::newton_d(p.a, p.gamma, [xp[0], xp[1], xp[2]]),
        CryptoMath::TwoV1 => crypto_v1_2::newton_d(p.a, p.gamma, [xp[0], xp[1]]),
        CryptoMath::TriV1 => crypto_v1_3::newton_d(p.a, p.gamma, [xp[0], xp[1], xp[2]]),
    }
}

/// xp: балансы в 18 знаках и ценах пула — xp[0] = b0·prec0, xp[k] = unsafe_div(b_k·price_scale[k−1]·prec_k, 10^18)
/// (Twocrypto L762-L765, Tricrypto L890-L896). None — переполнение.
pub fn scaled(p: &CryptoData, bal: &[U256]) -> Option<Vec<U256>> {
    let mut xp = Vec::with_capacity(bal.len());
    xp.push(mu(bal[0], p.precisions[0])?);
    for (k, b) in bal.iter().enumerate().skip(1) {
        // Порядок множителей — как в исходнике обеих формул: (баланс · price_scale) · precision.
        let v = mu(mu(*b, p.price_scale[k - 1])?, p.precisions[k])?;
        xp.push(udiv_u(v, E18));
    }
    Some(xp)
}

/// `_fee(xp)` — комиссия обмена над 10^10 при балансах xp. Две монеты — Twocrypto L1163-L1177 (f = fee_gamma·10^18 /
/// (fee_gamma + 10^18 − 4·10^18·x0/S·x1/S)), три — Tricrypto `_reduction_coefficient`. Итог — unsafe_div(mid·f +
/// out·(10^18 − f), 10^18). None — откат.
pub fn fee(p: &CryptoData, xp: &[U256]) -> Option<U256> {
    let f = if xp.len() == 2 {
        let s = au(xp[0], xp[1])?;
        let k = du(mu(du(mu(U256::from(4u8) * E18, xp[0])?, s)?, xp[1])?, s)?;
        du(mu(p.fee_gamma, E18)?, su(au(p.fee_gamma, E18)?, k)?)?
    } else {
        reduction(xp, p.fee_gamma)?
    };
    let mid = mu(p.mid_fee, f)?;
    let out = mu(p.out_fee, su(E18, f)?)?;
    Some(udiv_u(au(mid, out)?, E18))
}

/// Что делает: монет i за `amount` монеты j по внутренней цене пула price_scale (без обмена и комиссии) — оценка размера
/// пробы (решение 73.19), не сверяемая величина: xp_j = amount·prec_j·ps_j/10^18 (ps_0 = 10^18), монет = xp_j·10^18 /
/// (prec_i·ps_i). Принимает: состояние, индекс монеты i, индекс встречной j, сумму. Возвращает: сумму или None
/// (индекс вне пула, переполнение, деление на ноль). Ошибок нет.
pub fn spot(p: &CryptoData, i: usize, j: usize, amount: U256) -> Option<U256> {
    let xp = du(mu(mu(amount, *p.precisions.get(j)?)?, scale(p, j)?)?, E18)?;
    du(mu(xp, E18)?, mu(*p.precisions.get(i)?, scale(p, i)?)?)
}

/// Цена монеты k к монете 0 в 18 знаках: у монеты 0 — 10^18, у прочих — price_scale[k − 1]. None — индекс вне пула.
fn scale(p: &CryptoData, k: usize) -> Option<U256> {
    if k == 0 {
        Some(E18)
    } else {
        p.price_scale.get(k - 1).copied()
    }
}

/// `_reduction_coefficient(x, fee_gamma)` Tricrypto (L644-L663). None — откат.
fn reduction(x: &[U256], fee_gamma: U256) -> Option<U256> {
    let three = U256::from(3u8);
    let s = au(au(x[0], x[1])?, x[2])?;
    let mut k = du(mu(three * E18, x[0])?, s)?;
    k = udiv_u(mu(mu(k, three)?, x[1])?, s);
    k = udiv_u(mu(mu(k, three)?, x[2])?, s);
    if fee_gamma > U256::ZERO {
        k = du(mu(fee_gamma, E18)?, su(au(fee_gamma, E18)?, k)?)?;
    }
    Some(k)
}
