//! Файл: venues/bsc_dex/pools/load_balancer_v3_reclamm.rs
//! Что делает: решение 452 — виртуальные балансы пула ReClamm Balancer V3 на блоке из ответов пачки чтения (вид
//! computeCurrentVirtualBalances; у выпусков 20250409 / 20250702 — ещё getLastVirtualBalances и getCenterednessMargin)
//! и центрированность выпусков 1–2. Вынесено из load_balancer_v3_kinds.rs (предел размера файла). Без сети.
//! Кто вызывает: pools/load_balancer_v3_kinds.rs (decode).
//! От чего зависит: abi_balancer_v3_kinds.rs (IReClamm), balancer_v3_factories.rs (Release),
//! load_balancer_v3_hook_read.rs (Cursor), math/balancer_math.rs (divDown).
//! Настройки: нет.
//!
//! Источник: шапки abi_balancer_v3*.rs. ReClamm 20250409 / 20250702: computeCurrentVirtualBalances считает по
//! lastBalancesLiveScaled18 (getPoolTokenInfo), onSwap — по живым балансам обмена (ReClammPool.sol
//! _getRealAndVirtualBalances / onSwap); виртуальные балансы зависят от балансов только при пересчёте (ReClammMath.sol
//! computeCurrentVirtualBalances: обновление отношения цен — по времени, сдвиг диапазона — при центрированности <
//! границы; computeCenteredness выпусков 1–2: a·Vb vs Va·b, divDown). У 20260428 вид считает по
//! getCurrentLiveBalances — совпадает с onSwap.

use alloy::sol_types::SolCall;
use alloy_primitives::U256;

use super::super::math::balancer_math::div_down;
use super::abi_balancer_v3_kinds::IReClamm;
use super::balancer_v3_factories::Release;
use super::load_balancer_v3_hook_read::Cursor;

/// Виртуальные балансы ReClamm на блоке (см. шапку). Ответы выпуска читаются все (порядок calls) до решения.
/// Ошибки (текст): не по ABI, расчёт на живых балансах не сверен.
pub fn reclamm(
    cur: &mut Cursor,
    r: Release,
    live: &[U256],
    last_live: &[U256],
    initialized: bool,
) -> Result<(U256, U256), String> {
    let dec = |e: alloy::sol_types::Error| format!("ReClamm не по ABI: {e}");
    let view = cur.take();
    let (last, margin) = if r == Release::ReClamm {
        (Some(cur.take()), Some(cur.take()))
    } else {
        (None, None)
    };
    if !initialized {
        // Пул не инициализирован — обменов нет (paused в load_balancer_v3.rs), выпуск 3 откатывает вид.
        return Ok((U256::ZERO, U256::ZERO));
    }
    let v = IReClamm::computeCurrentVirtualBalancesCall::abi_decode_returns(view?).map_err(dec)?;
    if r == Release::ReClammLive || live == last_live {
        return Ok((v.currentVirtualBalanceA, v.currentVirtualBalanceB));
    }
    let (Some(last), Some(margin)) = (last, margin) else {
        return Err("ReClamm: нет ответов getLastVirtualBalances / getCenterednessMargin".into());
    };
    let last = IReClamm::getLastVirtualBalancesCall::abi_decode_returns(last?).map_err(dec)?;
    let margin = IReClamm::getCenterednessMarginCall::abi_decode_returns(margin?).map_err(dec)?;
    let (la, lb) = (last.lastVirtualBalanceA, last.lastVirtualBalanceB);
    if !v.changed && centeredness(live, la, lb).is_some_and(|c| c >= margin) {
        return Ok((la, lb));
    }
    Err("ReClamm (выпуск 2025): курсы токенов изменились с последней операции пула, а виртуальные балансы сдвигаются — \
         вид пула считает их по прежним балансам, onSwap — по живым; расчёт не сверен, котировки нет (решение 452)"
        .into())
}

/// computeCenteredness выпусков 20250409 / 20250702 (ReClammMath.sol L579-L605): a = 0 или b = 0 — 0; иначе меньшее из
/// a·Vb, Va·b divDown большее. None — переполнение / деление на ноль.
fn centeredness(live: &[U256], va: U256, vb: U256) -> Option<U256> {
    let (a, b) = (*live.first()?, *live.get(1)?);
    if a.is_zero() || b.is_zero() {
        return Some(U256::ZERO);
    }
    let num = a.checked_mul(vb)?;
    let den = va.checked_mul(b)?;
    if num <= den {
        div_down(num, den)
    } else {
        div_down(den, num)
    }
}
