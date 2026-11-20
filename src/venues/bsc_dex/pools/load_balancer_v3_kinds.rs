//! Файл: venues/bsc_dex/pools/load_balancer_v3_kinds.rs
//! Что делает: решение 452 — часть пачки чтения пула Balancer V3 на блоке, своя у выпуска и крючка (общая часть —
//! load_balancer_v3.rs): A стейблсвапа; веса на блоке и окно продажи LBP / QuantAMM; виртуальные балансы ReClamm;
//! параметры крючков StableSurge и MEV Capture; у MEV Capture — собирает ли ArbOS чаевые (ArbSys.arbOSVersion,
//! ArbOwnerPublic.getCollectTips). Разбор — в кривую, правило и крючок части V3. Без сети (вызовы и разбор зовут и тесты).
//! Кто вызывает: pools/load_balancer_v3.rs, тесты.
//! От чего зависит: abi_balancer_v3.rs, abi_balancer_v3_kinds.rs, balancer_v3_factories.rs, balancer_v3_hooks.rs,
//! load_balancer_v3_extra.rs, load_balancer_v3_fixed.rs, math/balancer_math.rs (divDown), core/types.
//! Настройки: нет (адреса прекомпайлов dex.arb_sys / dex.arb_owner_public передаёт load_balancer_v3.rs).
//!
//! Источник: шапки abi_balancer_v3*.rs. Чаевые: OffchainLabs/nitro@ccc5c828 arbos/tx_processor.go L995-L1035
//! (GASPRICE = GetPaidGasPrice: чаевые не собираются — базовая цена, т. е. tx.gasprice − block.basefee = 0 у крючка
//! MEV Capture; CollectTips: ArbOS 9 — да, ниже 60 — нет, с 60 — флаг getCollectTips). ReClamm 20250409 / 20250702:
//! computeCurrentVirtualBalances считает по lastBalancesLiveScaled18 (getPoolTokenInfo), onSwap — по живым балансам
//! обмена (ReClammPool.sol _getRealAndVirtualBalances / onSwap); виртуальные балансы зависят от балансов только при
//! пересчёте (ReClammMath.sol computeCurrentVirtualBalances: обновление отношения цен — по времени, сдвиг диапазона —
//! при центрированности < границы; computeCenteredness выпусков 1–2: a·Vb vs Va·b, divDown). У 20260428 вид считает по
//! getCurrentLiveBalances — совпадает с onSwap.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::pool_vault::{LbpState, LbpVer, V3Hook, VaultCurve, VaultV3Rule};

use super::super::math::balancer_math::div_down;
use super::abi_balancer_v3::IBalancerV3Pool as P;
use super::abi_balancer_v3_kinds::{
    IArbOwnerPublic, IArbSys, ILbp, IMevCaptureHook as M, IReClamm, IStableSurgeHook as S,
};
use super::balancer_v3_factories::Release;
use super::balancer_v3_hooks::HookKind;
use super::load_balancer_v3_extra::Extra;
use super::load_balancer_v3_fixed::Fixed;
use super::load_balancer_v3_hook_read::{Cursor, hook};

/// Вызовы выпуска и крючка (после общих шести). Принимает: пул, неизменяемое, адреса ArbSys и ArbOwnerPublic (нужны
/// только крючку MEV Capture). Ошибок нет.
pub fn calls(pool: Address, fx: &Fixed, pre: Option<(Address, Address)>) -> Vec<(Address, Bytes)> {
    let c = |to: Address, d: Vec<u8>| (to, Bytes::from(d));
    let mut l = Vec::new();
    let r = fx.release;
    if r.stable() {
        l.push(c(pool, P::getAmplificationParameterCall {}.abi_encode()));
    }
    if r.moving_weights() {
        l.push(c(pool, P::getNormalizedWeightsCall {}.abi_encode()));
    }
    if matches!(r, Release::LbpV1 | Release::LbpV2 | Release::LbpV3 | Release::LbpV4)
        || r == Release::FixedLbp
    {
        l.push(c(pool, ILbp::isSwapEnabledCall {}.abi_encode()));
    }
    if matches!(r, Release::ReClamm | Release::ReClammLive) {
        l.push(c(pool, IReClamm::computeCurrentVirtualBalancesCall {}.abi_encode()));
    }
    if r == Release::ReClamm {
        l.push(c(pool, IReClamm::getLastVirtualBalancesCall {}.abi_encode()));
        l.push(c(pool, IReClamm::getCenterednessMarginCall {}.abi_encode()));
    }
    match fx.hook {
        HookKind::None => {}
        HookKind::Surge(h) => {
            l.push(c(h, S::getMaxSurgeFeePercentageCall { pool }.abi_encode()));
            l.push(c(h, S::getSurgeThresholdPercentageCall { pool }.abi_encode()));
        }
        HookKind::Mev(h) => {
            l.push(c(h, M::isMevTaxEnabledCall {}.abi_encode()));
            l.push(c(h, M::getMaxMevSwapFeePercentageCall {}.abi_encode()));
            l.push(c(h, M::getPoolMevTaxMultiplierCall { pool }.abi_encode()));
            l.push(c(h, M::getPoolMevTaxThresholdCall { pool }.abi_encode()));
            if let Some((sys, owner)) = pre {
                l.push(c(sys, IArbSys::arbOSVersionCall {}.abi_encode()));
                l.push(c(owner, IArbOwnerPublic::getCollectTipsCall {}.abi_encode()));
            }
        }
    }
    l
}

/// Разбор ответов calls. Принимает: неизменяемое, ответы (по порядку calls), живые балансы, последние живые балансы
/// хранилища (getPoolTokenInfo lastBalancesLiveScaled18), инициализирован ли пул. Возвращает: кривую, правило, крючок.
/// Ошибки (текст): откат, не по ABI, точность A ≠ 1000, расчёт не сверен (ReClamm 1–2 при сдвиге, чаевые ArbOS).
pub fn decode(
    fx: &Fixed,
    got: &[Result<Bytes, String>],
    live: &[U256],
    last_live: &[U256],
    initialized: bool,
) -> Result<(VaultCurve, VaultV3Rule, V3Hook), String> {
    let mut cur = Cursor { got, at: 0 };
    let dec = |e: alloy::sol_types::Error| format!("ответ вида не по ABI: {e}");
    let r = fx.release;
    let amp = if r.stable() {
        let a = P::getAmplificationParameterCall::abi_decode_returns(cur.take()?).map_err(dec)?;
        if a.precision != U256::from(1_000u32) {
            return Err(format!("точность A {} ≠ 1000 (StableMath.sol L38)", a.precision));
        }
        a.value
    } else {
        U256::ZERO
    };
    let weights = if r.moving_weights() {
        P::getNormalizedWeightsCall::abi_decode_returns(cur.take()?).map_err(dec)?
    } else {
        Vec::new()
    };
    let lbp = matches!(r, Release::LbpV1 | Release::LbpV2 | Release::LbpV3 | Release::LbpV4);
    let enabled = if lbp || r == Release::FixedLbp {
        ILbp::isSwapEnabledCall::abi_decode_returns(cur.take()?).map_err(dec)?
    } else {
        false
    };
    let (curve, rule) = match (&fx.extra, r) {
        (Extra::Weighted { weights, min }, _) => {
            let rule = if r == Release::WeightedMin {
                VaultV3Rule::WeightedMin { min: min.clone() }
            } else {
                VaultV3Rule::Weighted
            };
            let w = weights.clone();
            (VaultCurve::Weighted { weights: w }, rule)
        }
        (Extra::None, Release::Stable | Release::StableOpen) => {
            (VaultCurve::Stable { amp }, VaultV3Rule::Stable)
        }
        (Extra::None, Release::StableGuard) => {
            (VaultCurve::Stable { amp }, VaultV3Rule::StableGuard)
        }
        (Extra::None, Release::ReClamm | Release::ReClammLive) => {
            let (va, vb) = reclamm(&mut cur, r, live, last_live, initialized)?;
            (VaultCurve::V3, VaultV3Rule::ReClamm { va, vb })
        }
        (Extra::Gyro2 { sqrt_alpha, sqrt_beta }, _) => {
            let (sqrt_alpha, sqrt_beta) = (*sqrt_alpha, *sqrt_beta);
            (VaultCurve::V3, VaultV3Rule::Gyro2Clp { sqrt_alpha, sqrt_beta })
        }
        (Extra::Eclp(p), _) => (VaultCurve::V3, VaultV3Rule::GyroEclp(p.clone())),
        (Extra::Lbp { project, block_in, virt, min }, _) => {
            let ver = match r {
                Release::LbpV3 => LbpVer::Virtual,
                Release::LbpV4 => LbpVer::VirtualMin,
                _ => LbpVer::Plain,
            };
            let l = LbpState {
                ver,
                enabled,
                project: *project,
                block_in: *block_in,
                virt: *virt,
                min: min.clone(),
            };
            (VaultCurve::Weighted { weights }, VaultV3Rule::Lbp(Box::new(l)))
        }
        (Extra::FixedLbp { project, rate }, _) => {
            let (project, rate) = (*project, *rate);
            (VaultCurve::V3, VaultV3Rule::FixedLbp { enabled, project, rate })
        }
        (Extra::QuantAmm { max_trade }, _) => {
            let rule = VaultV3Rule::QuantAmm { max_trade: *max_trade };
            (VaultCurve::Weighted { weights }, rule)
        }
        (x, r) => return Err(format!("неизменяемое {x:?} не того выпуска {r:?}")),
    };
    let hook = hook(fx.hook, &mut cur)?;
    Ok((curve, rule, hook))
}

/// Виртуальные балансы ReClamm на блоке (см. шапку). Ответы выпуска читаются все (порядок calls) до решения.
/// Ошибки (текст): не по ABI, расчёт на живых балансах не сверен.
fn reclamm(
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
