//! Файл: venues/bsc_dex/quote/vault_depth.rs
//! Что делает: глубина пула в общем хранилище (Balancer V2, решение 289; Balancer V3, решения 441 и 452) по направлению
//! в полосе цены (решение 73) — без комиссии пула (чистый объём, решение 73 п. 17): средняя цена исполнения не выходит
//! за полосу от цены малой пробы (покупка — не выше p0·(1+b), продажа — не ниже p0·(1−b)). Поиск суммы — делением
//! отрезка пополам по той же формуле, что котировка (math/balancer_math.rs с нулевой комиссией). Весовой пул не берёт
//! вход больше 30 % баланса (MAX_IN_RATIO) — тогда глубина до этого предела и full = false. Balancer V3 с крючком
//! StableSurge: статическая комиссия 0 не снимает надбавку всплеска (math/balancer_v3_hook.rs: 0 + наибольшая·часть),
//! поэтому для глубины крючок снимается — комиссии нет целиком (решение 73 п. 17). Пул StableSurge с onSwap только для
//! хранилища (open = false) не трогается: крючок откатывает обмен — пул неторгуемый, глубины нет. MEV Capture не
//! трогается (при нулевых чаевых комиссия и так статическая, 0).
//! Кто вызывает: quote/depth_band.rs (ветка VaultPool).
//! От чего зависит: core/types (VaultPoolData, V3Hook), math/balancer_math.rs.
//! Настройки: нет (полосу даёт вызывающий).

use alloy_primitives::U256;

use crate::core::types::pool_vault::{V3Hook, VaultPoolData};

use super::super::math::balancer_math;

/// Шагов деления пополам (точность — 2^-64 от верхней границы поиска); не настройка — свойство алгоритма поиска.
const STEPS: u32 = 64;
/// Доля баланса входа для пробы цены: 1/10^6 (малая сумма, по которой берётся цена p0 — сдвиг цены пула от неё много
/// меньше полосы dex.depth_band_pct). Свойство алгоритма оценки, не порог торговли. Её же берёт spot.rs (цена для
/// размера пробы), stable_depth.rs и solidly_depth.rs (старт поиска глубины, решение 304).
pub const PROBE_DIV: u64 = 1_000_000;
/// Потолок поиска глубины стейблсвапа — во столько раз больше баланса входа. Предела входа у StableMath в исходнике нет
/// (MAX_IN_RATIO 30 % — только у весового, WeightedMath.sol L36), поэтому граница поиска — свойство алгоритма (как 64
/// шага в stable_depth.rs, StableMath.sol L29-L33 — только пределы A и числа токенов): на потолке средняя цена ещё в
/// полосе — глубина до потолка и full = false.
const STABLE_CAP_BALANCES: u8 = 10;

/// Одно направление. Принимает: состояние пула, индексы входа и выхода, полосу b (0..1), покупка ли (вход — встречный
/// токен). Возвращает: (сумма встречного токена: покупка — вход, продажа — выход; полоса прочитана целиком) или None —
/// пул пуст / на паузе / расчёт невозможен.
pub fn side(d: &VaultPoolData, i: usize, j: usize, b: f64, buy: bool) -> Option<(U256, bool)> {
    if !(b > 0.0 && b < 1.0) || d.paused {
        return None;
    }
    let mut nf = d.clone();
    nf.fee = U256::ZERO;
    // Решение 73 п. 17: без всей комиссии — у StableSurge снять надбавку всплеска (open = false — откат, не трогать).
    if let Some(v3) = nf.v3.as_mut()
        && v3.open
        && matches!(v3.hook, V3Hook::Surge { .. })
    {
        v3.hook = V3Hook::None;
    }
    // Решение 473: у BPT линейного пула — виртуальный выпуск (в хранилище заранее выпущено 2^112 − 1).
    let bal_in = base(d, i)?;
    let usual = bal_in / U256::from(PROBE_DIV);
    let ranges = super::super::math::puzzle_ranges::active(&nf);
    let (probe, p_out) = if ranges {
        super::super::math::puzzle_ranges::probe(&nf, i, j, usual)?
    } else {
        if usual.is_zero() {
            return Some((U256::ZERO, true));
        }
        let out = balancer_math::amount_out(&nf, i, j, usual)?;
        if out.is_zero() {
            return Some((U256::ZERO, true));
        }
        (usual, out)
    };
    let bp = (b * 10_000.0) as u64;
    let lim = U256::from(if buy { 10_000 + bp } else { 10_000 - bp });
    let ten_k = U256::from(10_000u32);
    // Средняя цена «встречный за монету» в полосе? Покупка: in/out ≤ (p_in/p_out)·lim/1e4; продажа: out/in ≥ …·lim/1e4.
    let within = |x: U256| -> Option<bool> {
        let o = balancer_math::amount_out(&nf, i, j, x)?;
        if o.is_zero() {
            return Some(false);
        }
        Some(if buy {
            x.checked_mul(p_out)?.checked_mul(ten_k)? <= probe.checked_mul(o)?.checked_mul(lim)?
        } else {
            o.checked_mul(probe)?.checked_mul(ten_k)? >= p_out.checked_mul(x)?.checked_mul(lim)?
        })
    };
    // Верхняя граница поиска: весовой — 30 % баланса входа (больше пул откатит), стейблсвап — STABLE_CAP_BALANCES балансов.
    let cap = match &d.curve {
        _ if ranges => {
            super::super::math::puzzle_ranges::depth_cap(&nf, i, j, probe, p_out, lim, buy)?
        }
        crate::core::types::VaultCurve::Weighted { .. }
        | crate::core::types::VaultCurve::WeightedV2 { .. } => weighted_cap(d, i, bal_in)?,
        // Решение 473: managed — весовой (30 % баланса входа); стейблсвап с курсами и линейный — потолок стейблсвапа
        // (у LinearMath предела входа нет — сверх баланса выхода формула откатывается).
        crate::core::types::VaultCurve::V2(c) => match **c {
            crate::core::types::pool_vault::V2Curve::Managed { .. } => weighted_cap(d, i, bal_in)?,
            _ => bal_in.checked_mul(U256::from(STABLE_CAP_BALANCES))?,
        },
        // Решение 452: кривая V3 (Gyro, ReClamm, LBP с фиксированной ценой) — тот же потолок, что у стейблсвапа: сверх
        // ёмкости пула формула откатывается (котировки нет — «не в полосе»).
        crate::core::types::VaultCurve::Stable { .. }
        | crate::core::types::VaultCurve::StableV2 { .. }
        | crate::core::types::VaultCurve::V3 => {
            bal_in.checked_mul(U256::from(STABLE_CAP_BALANCES))?
        }
    };
    if within(cap).unwrap_or(false) {
        let o = balancer_math::amount_out(&nf, i, j, cap)?;
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
    let o = balancer_math::amount_out(&nf, i, j, lo)?;
    Some((if buy { lo } else { o }, true))
}

/// Потолок поиска весового пула — 30 % баланса входа (MAX_IN_RATIO; больше пул откатит). Решение 441 (ревью, п. 6): у
/// пула Balancer V3 предел — на живом балансе 18 знаков (WeightedMath V3 L173-L178: вход хранилища в 18 знаках с
/// курсом), в единицах токена — 30 % живого баланса / курс / множитель точности. Принимает: состояние, индекс входа,
/// сырой баланс входа. Возвращает: потолок в единицах токена. None — переполнение, нулевой курс или множитель.
fn weighted_cap(d: &VaultPoolData, i: usize, bal_in: U256) -> Option<U256> {
    let ratio = U256::from(3u8) * super::super::math::ten::E17;
    match &d.v3 {
        None => balancer_math::mul_down(bal_in, ratio),
        Some(v3) => {
            let live = balancer_math::mul_down(*v3.live.get(i)?, ratio)?;
            let per_rate = balancer_math::div_down(live, *v3.rates.get(i)?)?;
            per_rate.checked_div(*d.scaling.get(i)?)
        }
    }
}

/// Решение 473: база пробы и потолка поиска — баланс входа в хранилище; у BPT линейного пула — виртуальный выпуск
/// (выпуск − баланс BPT в хранилище, как считает пул: LinearPool.sol L747-L749), иначе проба от 2^112 − 1 даёт откат.
/// Принимает: состояние, индекс входа. Возвращает: базу в единицах токена. None — индекс вне пула, выпуск < баланса.
fn base(d: &VaultPoolData, i: usize) -> Option<U256> {
    let b = *d.balances.get(i)?;
    if let crate::core::types::VaultCurve::V2(c) = &d.curve
        && let crate::core::types::pool_vault::V2Curve::Linear { bpt, supply, .. } = &**c
        && usize::from(*bpt) == i
    {
        return supply.checked_sub(b);
    }
    Some(b)
}
