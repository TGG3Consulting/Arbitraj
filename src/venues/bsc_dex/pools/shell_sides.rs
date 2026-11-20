//! Файл: venues/bsc_dex/pools/shell_sides.rs
//! Что делает: решение 486 — сопоставление сторон пула Shell Protocol (id токенов в учёте xToken / yToken) с токенами
//! пары графа: id = keccak256(токен, 0) ERC-20 или id нативной монеты учёта («Ether») — тогда в паре стоит обёртка
//! dex.native_wrapped; и формула полезности реализации пула по спискам сверенных реализаций (dex.proteus_utility_v1 /
//! dex.proteus_utility_v2). Общее для проверки (verify_shell.rs) и чтения (load_shell.rs).
//! Кто вызывает: pools/{verify_shell, load_shell, shell_list}.rs.
//! От чего зависит: abi_shell.rs (ocean_id, ether_id), ctx.rs, core/types (ProteusUtility).
//! Настройки: dex.proteus_utility_v1, dex.proteus_utility_v2 [DEX]; dex.native_wrapped [bsc_dex] — передаёт вызывающий.

use std::str::FromStr;

use alloy_primitives::{Address, U256};

use crate::core::types::ProteusUtility;

use super::super::ctx::Ctx;
use super::abi_shell::{ether_id, ocean_id};

/// Сторона пула: токен пары и нативная ли она (id «Ether»).
pub type Side = (Address, bool);

/// Токен из списка кандидатов, чей id в учёте равен `id` (у обёртки — ещё id нативной монеты). Принимает: id, токены
/// кандидатов, обёртку (None — не задана). Возвращает: сторону или None — id не из кандидатов. Ошибок нет.
pub fn side(id: U256, candidates: &[Address], wrapped: Option<Address>) -> Option<Side> {
    if let Some(t) = candidates.iter().find(|t| ocean_id(**t) == id) {
        return Some((*t, false));
    }
    let w = wrapped.filter(|w| candidates.contains(w))?;
    (id == ether_id()).then_some((w, true))
}

/// Обе стороны пула среди токенов пары. Принимает: xToken, yToken, токены пары, обёртку. Возвращает: (x, y) или
/// причину (сторона не из пары; обе стороны — один токен пары: обёртка и нативная монета сразу).
pub fn sides(
    x_id: U256,
    y_id: U256,
    pair: (Address, Address),
    wrapped: Option<Address>,
) -> Result<(Side, Side), String> {
    let cands = [pair.0, pair.1];
    let x = side(x_id, &cands, wrapped);
    let y = side(y_id, &cands, wrapped);
    match (x, y) {
        (Some(x), Some(y)) if x.0 != y.0 => Ok((x, y)),
        (Some(_), Some(_)) => Err(format!(
            "обе стороны пула — один токен пары {:#x} (обёртка и нативная монета учёта)",
            pair.0
        )),
        _ => Err(format!(
            "id токенов пула в учёте {x_id:#x} / {y_id:#x} ≠ keccak256(токен, 0) пары {:#x} / {:#x} (и не нативная \
             монета учёта при dex.native_wrapped)",
            pair.0, pair.1
        )),
    }
}

/// Есть ли адрес в списке настройки DEX (адреса через запятую — как отдаёт ctx.dex_list). Ошибок нет: не задан — нет.
fn listed(ctx: &Ctx, name: &str, id: &str, a: Address) -> bool {
    ctx.dex_list(name, id)
        .unwrap_or_default()
        .iter()
        .any(|s| Address::from_str(s.trim()).is_ok_and(|x| x == a))
}

/// Формула полезности реализации: в dex.proteus_utility_v1 — V1, в dex.proteus_utility_v2 — V2. Принимает: окружение,
/// id DEX, адрес реализации. Возвращает: формулу или причину (реализации нет ни в одном списке или она в обоих —
/// исходник не сверен, расчёт был бы вслепую).
pub fn utility(ctx: &Ctx, dex: &str, implementation: Address) -> Result<ProteusUtility, String> {
    let v1 = listed(ctx, "dex.proteus_utility_v1", dex, implementation);
    let v2 = listed(ctx, "dex.proteus_utility_v2", dex, implementation);
    match (v1, v2) {
        (true, false) => Ok(ProteusUtility::V1),
        (false, true) => Ok(ProteusUtility::V2),
        (true, true) => Err(format!(
            "реализация {implementation:#x} в обоих списках dex.proteus_utility_v1 и _v2 DEX {dex}"
        )),
        (false, false) => Err(format!(
            "реализация пула {implementation:#x} не в списках dex.proteus_utility_v1 / _v2 DEX {dex} — исходник не \
             сверен, пул не считается"
        )),
    }
}
