//! Файл: venues/bsc_dex/pools/balancer_v3_hooks.rs
//! Что делает: решение 452 — какие крючки пулов Balancer V3 бот берёт. Хранилище зовёт крючок при обмене только по
//! флагам пула (Vault.sol 20241204-v3-vault L225-L284): beforeSwap / afterSwap — пул не берётся (официального крючка с
//! ними на Arbitrum нет, расчёт не сверен); динамическая комиссия — только крючки из списков DEX: StableSurge
//! (dex.bv3_stable_surge_hooks) и MEV Capture (dex.bv3_mev_capture_hooks), адреса — balancer-deployments output; пул,
//! чей onSwap закрыт для крючка (onlyVault), с StableSurge не берётся — каждый его обмен откатится (крючок сам зовёт
//! onSwap пула, StableSurgeHook.sol L136). Флаги крючка ликвидности и инициализации обмен не меняют (LBP и ReClamm —
//! сами себе крючок только по ним).
//! Кто вызывает: pools/load_balancer_v3_fixed.rs, pools/verify_balancer_v3.rs, тесты.
//! От чего зависит: ctx.rs (dex_list), abi_balancer_v3.rs (HooksConfig), balancer_v3_factories.rs (Release::open).
//! Настройки: dex.bv3_stable_surge_hooks, dex.bv3_mev_capture_hooks [DEX вида balancer_v3].

use std::str::FromStr;

use alloy_primitives::Address;

use super::super::ctx::Ctx;
use super::abi_balancer_v3::IBalancerV3Vault::HooksConfig;
use super::balancer_v3_factories::Release;

/// Крючок, считающий комиссию.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    /// Крючка обмена нет — статическая комиссия.
    None,
    /// StableSurge по адресу.
    Surge(Address),
    /// MEV Capture по адресу.
    Mev(Address),
}

/// Списки официальных крючков DEX.
#[derive(Debug, Clone, Default)]
pub struct Lists {
    pub surge: Vec<Address>,
    pub mev: Vec<Address>,
}

/// Списки из настроек. Ошибки (текст): список не задан (пустой — допустимо), не адрес.
pub fn lists(ctx: &Ctx, id: &str) -> Result<Lists, String> {
    let read = |name: &str| -> Result<Vec<Address>, String> {
        let list = ctx.dex_list(name, id).map_err(|e| e.message)?;
        list.iter()
            .map(|s| {
                Address::from_str(s.trim())
                    .map_err(|_| format!("{name} [dex:{id}]: «{s}» — не адрес"))
            })
            .collect()
    };
    Ok(Lists {
        surge: read("dex.bv3_stable_surge_hooks")?,
        mev: read("dex.bv3_mev_capture_hooks")?,
    })
}

/// Вид крючка пула. Принимает: списки, настройки крючков пула (getHooksConfig), выпуск пула. Возвращает: вид или
/// причину отказа (текст). Ошибок иных нет.
pub fn kind(l: &Lists, h: &HooksConfig, r: Release) -> Result<HookKind, String> {
    let x = h.hooksContract;
    if let Release::PuzzleRanges(expected) = r {
        if x == expected && !x.is_zero() && !h.enableHookAdjustedAmounts
            && !h.shouldCallComputeDynamicSwapFee && !h.shouldCallBeforeSwap && h.shouldCallAfterSwap {
            return Ok(HookKind::None);
        }
        return Err(format!("Puzzle Ranges: другой контракт или режим крючка {x:#x}"));
    }
    if h.shouldCallBeforeSwap || h.shouldCallAfterSwap {
        return Err(format!(
            "у пула крючок {x:#x} до / после обмена (beforeSwap {}, afterSwap {}) — расчёт не сверен, пул не берётся",
            h.shouldCallBeforeSwap, h.shouldCallAfterSwap
        ));
    }
    if !h.shouldCallComputeDynamicSwapFee {
        return Ok(HookKind::None);
    }
    if l.surge.contains(&x) {
        if !r.open() {
            return Err(format!(
                "крючок StableSurge {x:#x} у пула выпуска {r:?}, чей onSwap открыт только хранилищу (onlyVault) — \
                 каждый обмен откатится"
            ));
        }
        return Ok(HookKind::Surge(x));
    }
    if l.mev.contains(&x) {
        return Ok(HookKind::Mev(x));
    }
    Err(format!(
        "у пула крючок динамической комиссии {x:#x} не из dex.bv3_stable_surge_hooks / dex.bv3_mev_capture_hooks — \
         расчёт не сверен, пул не берётся"
    ))
}
