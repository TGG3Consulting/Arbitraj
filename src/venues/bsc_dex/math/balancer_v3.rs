//! Файл: venues/bsc_dex/math/balancer_v3.rs
//! Что делает: решение 441 (T433-F2) — обмен в пуле Balancer V3 так, как его считает хранилище Vault V3 с вызовом
//! onSwap пула (пул без крючков обмена — их проверяет pools/verify_balancer_v3.rs): сумма → 18 знаков с курсом токена,
//! комиссия (точный вход — со входа mulUp; точный выход — поверх mulDivUp), наименьшая сумма обмена, формула пула по
//! правилу его фабрики (весовая — как V2 с powUp V3, стейблсвап — math/balancer_v3_stable.rs), обратно в единицы токена
//! с округлением хранилища.
//! Решение 452: формула пула — math/balancer_v3_pool.rs (все виды), комиссия — math/balancer_v3_hook.rs (статическая
//! или крючка StableSurge / MEV Capture), выход не больше сырого баланса хранилища (Vault.sol L466-L470).
//! Решение 474: сторона пары через буфер ERC4626 (VaultV3.via) — до обмена WRAP входа, после — UNWRAP выхода
//! (math/balancer_v3_buffer.rs); точный выход через буфер не считается (нужны previewMint / previewWithdraw обёртки —
//! бот их не читает): amount_in — None.
//! Кто вызывает: math/balancer_math.rs (amount_out / amount_in — ветка состояния с частью V3).
//! От чего зависит: core/types (VaultPoolData, VaultV3, VaultV3Rule, VaultCurve), math/balancer_math.rs (FixedPoint),
//! math/balancer_v3_weighted.rs (powUp V3 и WeightedMath), math/balancer_v3_stable.rs (StableMath V3).
//! Настройки: нет (состояние — pools/load_balancer_v3.rs).
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8, build-info (исходники развёрнутого кода):
//! - v3/tasks/20241204-v3-vault → contracts/Vault.sol: swap L197-L291 (крючки — только при флагах пула),
//!   _computeAmountGivenScaled18 L326-L346, _swap L368-L430 (комиссия, _ensureValidSwapAmount до и после onSwap,
//!   обратный пересчёт); L1554-L1558 _ensureValidSwapAmount (сумма < _MINIMUM_TRADE_AMOUNT — откат);
//!   v3-solidity-utils/helpers/ScalingHelpers.sol L38-L44 (toScaled18ApplyRateRoundDown), L56-L62 (…RoundUp),
//!   L74-L82 (toRawUndoRateRoundDown = divDown(сумма, множитель·курс)), L94-L102 (…RoundUp = divUp), L212-L220
//!   (computeRateRoundUp); FixedPoint.sol L20-L36 (mulDown, mulUp), L38-L48 (divDown, divUp), L51-L71 (mulDivUp),
//!   L124-L140 (powUp: y = 1 → x, y = 2 → mulUp(x, x), y = 4 → mulUp квадрата, иначе pow + mulUp(pow, 1e4) + 1),
//!   L148-L154 (complement);
//! - весовой: v3/deprecated/20241205-v3-weighted-pool → contracts/WeightedPool.sol onSwap L150-L176;
//!   v3/tasks/20260115-v3-weighted-pool-v2 → WeightedPool.sol onSwap L172-L208 (баланс входа + 1, нижние балансы
//!   _ensureMinimumBalance L378-L397), WeightedMath.sol computeOutGivenExactIn L173-L208, computeInGivenExactOut L219-L255
//!   (MAX_IN_RATIO = MAX_OUT_RATIO = 30 %, L54-L55; у 20241205 — те же формулы, L171-L205 / L216-L255);
//! - стейблсвап: v3/deprecated/20241205-v3-stable-pool → StablePool.sol computeInvariant L139-L148 (ROUND_DOWN — без
//!   +1), onSwap L169-L196; v3/tasks/20260116-v3-stable-pool-v3 → StablePool.sol onSwap L195-L247 (проверка дисбаланса).

use alloy_primitives::U256;

use crate::core::types::pool_vault::{VaultPoolData, VaultV3};

use super::balancer_math::{div_down, div_up, mul_down, mul_up, one};
use super::balancer_v3_buffer;
use super::balancer_v3_hook;
use super::balancer_v3_pool::on_swap;
use super::balancer_v3_weighted::{complement, mul_div_up};

/// Курс с округлением вверх (computeRateRoundUp, ScalingHelpers.sol L212-L220). Ошибок нет.
fn rate_up(r: U256) -> U256 {
    if (r / one()) * one() == r {
        r
    } else {
        r + U256::from(1u8)
    }
}

/// Решение 474: буфер стороны пары с индексом токена пула k (None — сторона — сам токен пула). Ошибок нет.
fn via(v3: &VaultV3, k: usize) -> Option<&crate::core::types::pool_vault::V3Buffer> {
    v3.via.get(k).and_then(|b| b.as_ref())
}

/// Выход на точный вход (Vault.swap EXACT_IN). Принимает: состояние пула, его часть V3, индексы входа и выхода, вход в
/// единицах монеты пары (решение 474: у стороны через буфер — базового токена обёртки). Возвращает: выход в единицах
/// монеты пары. None — пауза, индексы, сумма меньше наименьшей, откат пула, буфер (balancer_v3_buffer.rs).
pub fn amount_out(
    d: &VaultPoolData,
    v3: &VaultV3,
    i: usize,
    j: usize,
    amount_in: U256,
) -> Option<U256> {
    if d.paused || i == j || amount_in.is_zero() {
        return None;
    }
    // Решение 474: WRAP входа в буфере (Vault.sol L1139-L1187), обмен — по обёртке, UNWRAP выхода.
    let amount_in = match via(v3, i) {
        Some(b) => balancer_v3_buffer::wrap(b, amount_in)?,
        None => amount_in,
    };
    let out = pool_out(d, v3, i, j, amount_in)?;
    match via(v3, j) {
        Some(b) => balancer_v3_buffer::unwrap(b, out),
        None => Some(out),
    }
}

/// Выход обмена в самом пуле (токены пула). Принимает и возвращает — как amount_out, в единицах токенов пула.
fn pool_out(d: &VaultPoolData, v3: &VaultV3, i: usize, j: usize, amount_in: U256) -> Option<U256> {
    // _computeAmountGivenScaled18 (L333-L337): (сумма · множитель).mulDown(курс).
    let scaled = mul_down(amount_in.checked_mul(*d.scaling.get(i)?)?, *v3.rates.get(i)?)?;
    // Решение 452: комиссия — статическая или крючка (Vault.sol L246-L253: крючок видит полную сумму до комиссии).
    let fee = balancer_v3_hook::fee(d, v3, i, j, scaled, true)?;
    // _swap L386-L390: комиссия mulUp со входа.
    let net = scaled.checked_sub(mul_up(scaled, fee)?)?;
    if net < v3.min_trade {
        return None;
    }
    let out18 = on_swap(&d.curve, v3, i, j, net, true)?;
    if out18 < v3.min_trade {
        return None;
    }
    // L406-L414: toRawUndoRateRoundDown(выход, множитель, computeRateRoundUp(курс)).
    let den = d.scaling.get(j)?.checked_mul(rate_up(*v3.rates.get(j)?))?;
    let out = div_down(out18, den)?;
    // Решение 452: L466-L470 — сырой баланс выхода − выход (вычитание с откатом). Сырой баланс — на блоке чтения (сбор
    // доходности при обмене может его уменьшить; у весового и стейблсвапа предел не достигается — формула откатится
    // раньше).
    (out <= *d.balances.get(j)?).then_some(out)
}

/// Вход на точный выход (Vault.swap EXACT_OUT). Принимает: состояние, часть V3, индексы, выход в единицах токена.
/// Возвращает: вход в единицах токена с комиссией. None — как amount_out.
pub fn amount_in(
    d: &VaultPoolData,
    v3: &VaultV3,
    i: usize,
    j: usize,
    amount_out: U256,
) -> Option<U256> {
    // Решение 474: точный выход через буфер не считается (шапка файла).
    if d.paused || i == j || amount_out.is_zero() || via(v3, i).is_some() || via(v3, j).is_some() {
        return None;
    }
    // Решение 452: выход больше сырого баланса — откат хранилища (L466-L470).
    if amount_out > *d.balances.get(j)? {
        return None;
    }
    // L338-L345: (сумма · множитель).mulUp(computeRateRoundUp(курс выхода)).
    let given = amount_out.checked_mul(*d.scaling.get(j)?)?;
    let scaled = mul_up(given, rate_up(*v3.rates.get(j)?))?;
    if scaled < v3.min_trade {
        return None;
    }
    let fee = balancer_v3_hook::fee(d, v3, i, j, scaled, false)?;
    let in18 = on_swap(&d.curve, v3, i, j, scaled, false)?;
    if in18 < v3.min_trade {
        return None;
    }
    // L419-L427: комиссия mulDivUp(вход, комиссия, 1 − комиссия), затем toRawUndoRateRoundUp(…, курс входа).
    let charge = mul_div_up(in18, fee, complement(fee))?;
    let total = in18.checked_add(charge)?;
    let den = d.scaling.get(i)?.checked_mul(*v3.rates.get(i)?)?;
    div_up(total, den)
}
