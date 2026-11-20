//! Файл: venues/bsc_dex/pools/lb_hooks.rs
//! Что делает: решение 446 — крючки пула Liquidity Book V2.2. У пула V2.2 есть слово _hooksParameters: младшие 160 бит —
//! адрес контракта крючков, бит 160 — beforeSwap, бит 161 — afterSwap (остальные флаги — mint, burn, заём, перевод
//! долей — обмен не трогают). При флаге обмена пул в swap зовёт крючок (call со всем газом) и откатывает обмен, если
//! крючок откатился или вернул не тот селектор. Расчёт выхода крючок не меняет: beforeSwap зовётся после подсчёта входа
//! и до чтения бинов, пул в это время закрыт от повторного входа; afterSwap — после выплаты. Но код крючка — чужой:
//! он может откатить обмен (белый список) и тратит газ. Поэтому пул с флагом обмена берётся, только если адрес его
//! крючка — в dex.lb_hooks_allowed (не задана — такие пулы не берутся, причина видна). Флаги без обмена — пул берётся.
//! Крючки меняет только фабрика (setHooksParameters → событие HooksParametersSet; events_lb22.rs перечитывает пул, а
//! чтение снова проходит эту проверку).
//! Кто вызывает: pools/load.rs (ветка LB V2.2: перед чтением состояния), pools/verify_lb22.rs (decide).
//! От чего зависит: ctx.rs, node/call.rs, abi_lb22.rs, families.rs.
//! Настройки: dex.lb_hooks_allowed [DEX вида lb_v22].
//!
//! Источник: lfj-gg/joe-v2@v2.2.0 (1297c38) — src/libraries/Hooks.sol L13-L14 (BEFORE_SWAP_FLAG = 1 << 160,
//! AFTER_SWAP_FLAG = 1 << 161), L83-L85 (getHooks — младшие 160 бит), L129-L156 (beforeSwap / afterSwap — только при
//! флаге), L350-L369 (_safeCall: откат крючка — откат обмена; ответ — селектор функции); src/LBPair.sol L493-L579 (swap:
//! вход L501, beforeSwap L506, цикл по бинам L517-L560, выплата L570-L574, afterSwap L578), L846-L861 (setHooksParameters).

use std::str::FromStr;

use alloy_primitives::{Address, B256, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_lb22::ILBPair22;
use super::families::Family;

/// Флаги обмена: биты 160 (beforeSwap) и 161 (afterSwap) — Hooks.sol L13-L14.
fn swap_flags(p: B256) -> (bool, bool) {
    let w = U256::from_be_bytes(p.0);
    (w.bit(160), w.bit(161))
}

/// Адрес крючков — младшие 160 бит слова (Hooks.getHooks, L83-L85). Ошибок нет.
pub fn hooks_address(p: B256) -> Address {
    Address::from_slice(&p.0[12..])
}

/// Решение по слову крючков (без сети; его же зовут тесты на записанных ответах). Принимает: слово
/// getLBHooksParameters, допущенные крючки (dex.lb_hooks_allowed; None — настройка не задана). Возвращает: Ok — пул
/// берётся; Err — причина (флаг обмена при крючке не из списка или при не заданной настройке).
pub fn decide(p: B256, allowed: Option<&[Address]>) -> Result<(), String> {
    let (before, after) = swap_flags(p);
    if !before && !after {
        return Ok(());
    }
    let hooks = hooks_address(p);
    let which = match (before, after) {
        (true, true) => "beforeSwap и afterSwap",
        (true, false) => "beforeSwap",
        _ => "afterSwap",
    };
    match allowed {
        Some(list) if list.contains(&hooks) => Ok(()),
        Some(_) => Err(format!(
            "крючок обмена {hooks:#x} ({which}) не в dex.lb_hooks_allowed — пул не берётся"
        )),
        None => Err(format!(
            "крючок обмена {hooks:#x} ({which}), dex.lb_hooks_allowed не задана — пул не берётся"
        )),
    }
}

/// Допущенные крючки DEX. Возвращает: Ok(None) — не задана; Ok(Some(список)). Ошибка — элемент не адрес.
pub fn allowed(ctx: &Ctx, f: &Family) -> Result<Option<Vec<Address>>, String> {
    let Ok(list) = ctx.dex_list("dex.lb_hooks_allowed", f.id) else {
        return Ok(None);
    };
    list.iter()
        .map(|s| {
            Address::from_str(s.trim())
                .map_err(|_| format!("dex.lb_hooks_allowed: «{s}» — не адрес"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// Проверка крючков пула на блоке чтения. Принимает: окружение, вид, пул, блок (тег). Ошибки: узел (вызов не прошёл),
/// BadResponse (ответ не по ABI, крючок обмена не допущен, список не из адресов).
pub async fn check(ctx: &Ctx, f: &Family, pool: Address, tag: &str) -> GatewayResult<()> {
    let p = call::view(ctx, pool, &ILBPair22::getLBHooksParametersCall {}, tag).await?;
    let bad = |why: String| ctx.err(ErrorCode::BadResponse, format!("LB V2.2 {pool:#x}: {why}"));
    let list = allowed(ctx, f).map_err(&bad)?;
    decide(p, list.as_deref()).map_err(bad)
}
