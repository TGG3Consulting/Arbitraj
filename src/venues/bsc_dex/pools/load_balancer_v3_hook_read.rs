//! Файл: venues/bsc_dex/pools/load_balancer_v3_hook_read.rs
//! Что делает: решение 452 — разбор ответов пачки чтения пула Balancer V3 по порядку (Cursor) и параметров крючка
//! комиссии на блоке: StableSurge (наибольшая комиссия, порог), MEV Capture (включён ли налог, наибольшая комиссия,
//! множитель и порог пула) и чаевые, которые увидит крючок MEV Capture: ArbOS не собирает чаевые — 0. Вынесено из
//! load_balancer_v3_kinds.rs (предел размера файла).
//! Кто вызывает: pools/load_balancer_v3_kinds.rs.
//! От чего зависит: abi_balancer_v3_kinds.rs, balancer_v3_hooks.rs, core/types (V3Hook).
//! Настройки: нет.
//!
//! Источник: balancer-deployments@b55dc2b8 build-info 20250403-v3-stable-surge-hook-v2 StableSurgeHook.sol L94-L101
//! (getMaxSurgeFeePercentage / getSurgeThresholdPercentage), 20250212-v3-mev-capture-hook MevCaptureHook.sol L149-L221
//! (isMevTaxEnabled, getMaxMevSwapFeePercentage, getPoolMevTaxMultiplier, getPoolMevTaxThreshold), L353-L355
//! (чаевые = tx.gasprice − block.basefee); OffchainLabs/nitro@ccc5c828 arbos/tx_processor.go L995-L1035 (GASPRICE —
//! базовая цена, пока чаевые не собираются; CollectTips: ArbOS 9 — да, ниже 60 — нет, с 60 — флаг);
//! nitro-precompile-interfaces@72f30751 ArbSys.sol L33-L38 (arbOSVersion = 55 + версия), ArbOwnerPublic.sol L90-L92.

use alloy::sol_types::SolCall;
use alloy_primitives::{Bytes, U256};

use crate::core::types::pool_vault::V3Hook;

use super::abi_balancer_v3_kinds::{
    IArbOwnerPublic, IArbSys, IMevCaptureHook as M, IStableSurgeHook as S,
};
use super::balancer_v3_hooks::HookKind;

/// Сдвиг arbOSVersion к версии ArbOS (ArbSys.sol L33-L38: «55 + версия»).
const ARBOS_OFFSET: u64 = 55;

/// Ответы пачки по порядку calls.
pub struct Cursor<'a> {
    pub got: &'a [Result<Bytes, String>],
    pub at: usize,
}

impl<'a> Cursor<'a> {
    /// Следующий ответ. Ошибки (текст): откат вызова, нет ответа.
    pub fn next(&mut self) -> Result<&'a Bytes, String> {
        let i = self.at;
        self.at += 1;
        match self.got.get(i) {
            Some(Ok(b)) => Ok(b),
            Some(Err(w)) => Err(format!("вызов вида {i} откатился: {w}")),
            None => Err(format!("нет ответа на вызов вида {i}")),
        }
    }
}

/// Параметры крючка комиссии на блоке. Ошибки (текст): не по ABI, ArbOS собирает чаевые (комиссия зависит от чаевых
/// сделки — расчёт не задан), версия ArbOS не прочитана.
pub fn hook(k: HookKind, cur: &mut Cursor) -> Result<V3Hook, String> {
    let dec = |e: alloy::sol_types::Error| format!("крючок не по ABI: {e}");
    Ok(match k {
        HookKind::None => V3Hook::None,
        HookKind::Surge(_) => V3Hook::Surge {
            max: S::getMaxSurgeFeePercentageCall::abi_decode_returns(cur.next()?).map_err(dec)?,
            threshold: S::getSurgeThresholdPercentageCall::abi_decode_returns(cur.next()?)
                .map_err(dec)?,
        },
        HookKind::Mev(_) => {
            let enabled = M::isMevTaxEnabledCall::abi_decode_returns(cur.next()?).map_err(dec)?;
            let max = M::getMaxMevSwapFeePercentageCall::abi_decode_returns;
            let max = max(cur.next()?).map_err(dec)?;
            let mult = M::getPoolMevTaxMultiplierCall::abi_decode_returns;
            let mult = mult(cur.next()?).map_err(dec)?;
            let thr = M::getPoolMevTaxThresholdCall::abi_decode_returns(cur.next()?).map_err(dec)?;
            let ver = IArbSys::arbOSVersionCall::abi_decode_returns(cur.next()?).map_err(dec)?;
            let tips = cur.next();
            let ver = ver.saturating_sub(U256::from(ARBOS_OFFSET));
            // tx_processor.go L995-L1015: ArbOS 9 — чаевые собираются, ниже 60 — нет, с 60 — флаг getCollectTips.
            let collect = if ver == U256::from(9u8) {
                true
            } else if ver < U256::from(60u8) {
                false
            } else {
                IArbOwnerPublic::getCollectTipsCall::abi_decode_returns(tips?).map_err(dec)?
            };
            if collect {
                return Err(format!(
                    "ArbOS {ver} собирает чаевые: комиссия крючка MEV Capture зависит от чаевых сделки — расчёт не \
                     задан, котировки нет (вопрос в docs/TASKS.md, решение 452)"
                ));
            }
            V3Hook::Mev {
                enabled,
                max,
                multiplier: mult,
                threshold: thr,
                priority: U256::ZERO,
            }
        }
    })
}
