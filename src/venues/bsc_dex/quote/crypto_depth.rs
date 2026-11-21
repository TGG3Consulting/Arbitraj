//! Файл: venues/bsc_dex/quote/crypto_depth.rs
//! Что делает: глубина crypto-пула Curve (решение 442: Twocrypto-NG, Tricrypto-NG) по направлению в полосе цены
//! (решение 73) — без комиссии пула (чистый объём, решение Тиграна к 73): средняя цена исполнения не выходит за полосу
//! от цены малой пробы (покупка — не выше p0·(1+b), продажа — не ниже p0·(1−b)). Поиск суммы — делением отрезка
//! пополам по той же формуле, что котировка (math/crypto_swap.rs с нулевыми mid_fee / out_fee). Где формула пула
//! откатилась бы (assert'ы get_y на краях), — «не в полосе»: глубина до этой точки.
//! Кто вызывает: quote/depth_band.rs (ветка Crypto).
//! От чего зависит: core/types (CryptoData), math/crypto_swap.rs, quote/vault_depth.rs (PROBE_DIV).
//! Настройки: нет (полосу даёт вызывающий).

use alloy_primitives::U256;

use crate::core::types::CryptoData;

use super::super::math::crypto_swap;
use super::vault_depth::PROBE_DIV;

/// Шагов деления пополам (точность — 2^-64 от верхней границы поиска); не настройка — свойство алгоритма поиска.
const STEPS: u32 = 64;
/// Потолок поиска — во столько раз больше баланса входа (свойство алгоритма, как у стейблсвапа vault_depth.rs): на
/// потолке средняя цена ещё в полосе — глубина до потолка и full = false.
const CAP_BALANCES: u8 = 10;

/// Одно направление. Принимает: состояние пула, индексы входа и выхода, полосу b (0..1), покупка ли (вход — встречный
/// токен). Возвращает: (сумма встречного токена: покупка — вход, продажа — выход; полоса прочитана целиком) или None —
/// пул остановлен / расчёт невозможен.
pub fn side(d: &CryptoData, i: usize, j: usize, b: f64, buy: bool) -> Option<(U256, bool)> {
    if !(b > 0.0 && b < 1.0) || d.halted {
        return None;
    }
    let mut nf = d.clone();
    nf.mid_fee = U256::ZERO;
    nf.out_fee = U256::ZERO;
    // Решение 472: без комиссии virtual_price не растёт — модель tweak_price (откат обмена) к чистому объёму не
    // применяется; откат реального обмена проверяет котировка (pool_quote.rs → crypto_swap::exchange_out).
    nf.tweak = None;
    let out = |x: U256| crypto_swap::exchange_out(&nf, i, j, x);
    let bal_in = *d.balances.get(i)?;
    let probe = bal_in / U256::from(PROBE_DIV);
    if probe.is_zero() {
        return Some((U256::ZERO, true));
    }
    let p_out = out(probe)?;
    if p_out.is_zero() {
        return Some((U256::ZERO, true));
    }
    let bp = (b * 10_000.0) as u64;
    let lim = U256::from(if buy { 10_000 + bp } else { 10_000 - bp });
    let ten_k = U256::from(10_000u32);
    // Средняя цена «встречный за монету» в полосе? Покупка: in/out ≤ (p_in/p_out)·lim/1e4; продажа: out/in ≥ …·lim/1e4.
    let within = |x: U256| -> Option<bool> {
        let o = out(x)?;
        if o.is_zero() {
            return Some(false);
        }
        Some(if buy {
            x.checked_mul(p_out)?.checked_mul(ten_k)? <= probe.checked_mul(o)?.checked_mul(lim)?
        } else {
            o.checked_mul(probe)?.checked_mul(ten_k)? >= p_out.checked_mul(x)?.checked_mul(lim)?
        })
    };
    let cap = bal_in.checked_mul(U256::from(CAP_BALANCES))?;
    if within(cap).unwrap_or(false) {
        let o = out(cap)?;
        return Some((if buy { cap } else { o }, false));
    }
    let (mut lo, mut hi) = (probe, cap);
    if !within(lo).unwrap_or(false) {
        return Some((U256::ZERO, true));
    }
    for _ in 0..STEPS {
        if hi <= lo + U256::from(1u8) {
            break;
        }
        let mid = lo + (hi - lo) / U256::from(2u8);
        if within(mid).unwrap_or(false) {
            lo = mid
        } else {
            hi = mid
        }
    }
    let o = out(lo)?;
    Some((if buy { lo } else { o }, true))
}
