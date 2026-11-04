//! Файл: venues/bsc_dex/quote/graph_executor.rs
//! Что делает: правила исполнимости графового маршрута через свой исполнитель ArbExecutor (решение 122, часть X;
//! вынесено из quote/graph_exec.rs): каждый шаг — вид шага исполнителя при заданном exec.executor_address (пометка
//! части F, pools/exec_kind.rs) и пул со своим контрактом; форма Universal Router (прямая / умный путь) исполнителем
//! не идёт; монета с налогом на входе — только один первый шаг «вход переводом в пул» на весь вход (ревью 122, п. 3).
//! Кто вызывает: quote/graph_exec.rs (executable), тесты.
//! От чего зависит: pools/{exec_kind, families}.rs, exec/executor/entry.rs, quote/path.rs.
//! Настройки: exec.executor_address [bsc_dex] (через exec_kind::via_executor).

use super::super::ctx::Ctx;
use super::super::exec::executor::entry;
use super::super::pools::exec_kind::{self, ExecKind};
use super::super::pools::families::smart_ok;
use super::path::Path;

/// Пойдёт ли маршрут через исполнитель: у каждого шага вид шага исполнителя при заданном адресе
/// (pools/exec_kind.rs::via_executor — пометка части F) и у пула свой контракт (exec/executor/entry.rs::supported).
/// Ошибок нет.
pub fn all_via(ctx: &Ctx, legs: &[Path]) -> bool {
    legs.iter().flat_map(|p| p.hops.iter()).all(|h| {
        exec_kind::via_executor(ctx, h.entry.family)
            && entry::supported(h.entry.family, &h.entry.pool.reference).is_ok()
    })
}

/// Форма Universal Router (как graph_form: прямая или умный путь) — тогда исполнитель не используется. Ошибок нет.
pub fn ur_form(ctx: &Ctx, legs: &[Path]) -> bool {
    let hops = || legs.iter().flat_map(|p| p.hops.iter());
    if hops().any(|h| h.entry.family.fork) {
        return false;
    }
    let single = legs.len() == 1 && legs[0].hops.len() == 1;
    (single && hops().all(|h| h.entry.family.tradable))
        || hops().all(|h| smart_ok(ctx, h.entry.family))
}

/// Налоговая ли монета на ВХОДЕ маршрута (решение 146): у первого шага какой-либо доли монета пула — вход шага (продажа
/// монеты пула) и налог продажи ИМЕННО этого пула > 0 (Hop.usable.tax_pct). Ошибок нет.
pub fn taxed_in(legs: &[Path]) -> bool {
    legs.iter()
        .filter_map(|p| p.hops.first())
        .any(|h| h.token_in == h.entry.coin && h.usable.tax_pct > rust_decimal::Decimal::ZERO)
}

/// Ревью 122, пункт 3: монета с налогом на ВХОДЕ через исполнитель платит налог один раз, только если весь вход берёт
/// единственный первый шаг вида «вход переводом в пул» (V2 / SOLIDLY / DODO / WOO — прямой вход владелец → пул,
/// contracts/README.md). Иначе вход идёт кошелёк → исполнитель → пул (налог дважды; V3 / ALGEBRA / MAVERICK пул
/// откатит обмен по недобору) — неисполнимо, сигнал с причиной. Налоговость входа — по налогу пула первого шага
/// (taxed_in, решение 146). Принимает: доли.
pub fn tax(legs: &[Path]) -> Result<(), String> {
    if !taxed_in(legs) {
        return Ok(());
    }
    if legs.len() > 1 {
        return Err(format!(
            "у монеты налог на входе: через исполнитель маршрут из {} долей платит налог дважды (кошелёк → исполнитель → \
             пул) — сигнал",
            legs.len()
        ));
    }
    let first = legs.first().and_then(|p| p.hops.first());
    match first.and_then(|h| h.entry.family.executor_kind) {
        Some(ExecKind::V2 | ExecKind::Solidly | ExecKind::Dodo | ExecKind::Woo) => Ok(()),
        k => Err(format!(
            "у монеты налог на входе: первый шаг исполнителя {} — вход через исполнитель (налог дважды, пул откатит \
             обмен); прямой вход только у V2 / SOLIDLY / DODO / WOO — сигнал",
            k.map(ExecKind::name).unwrap_or("—")
        )),
    }
}
