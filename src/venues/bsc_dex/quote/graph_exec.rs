//! Файл: venues/bsc_dex/quote/graph_exec.rs
//! Что делает: исполнимость графового маршрута как есть (решение 84; вынесено из quote/graph_route.rs) и его форма для
//! боевого пути: прямая (одна доля, один хоп), всё через один Universal Router PancakeSwap (умный путь) или
//! мульти-роутерный бандл свопов (exec/bundle.rs). Неисполнимый маршрут — сигнал с причиной (exec_note).
//! Решение 122, часть R2: нога из N шагов внутри ОДНОГО Universal Router (PancakeSwap: V2/V3/Infinity; Uniswap:
//! V2/V3/V4) исполнима одной транзакцией — цепочка команд UR, промежуточный токен остаётся на роутере (ADDRESS_THIS,
//! вход следующей команды — CONTRACT_BALANCE; exec/chain.rs). Прежнего жёсткого предела «≤ 2 хопов» нет; предел
//! длины ноги — dex.route_max_hops (тот же, что у перебора путей). Нога, пересекающая роутеры (разные семейства), —
//! по-прежнему сигнал (бандл такую ногу не собирает; позже — свой исполнитель, решение 122 часть X).
//! Решение 122, часть R3: нативная монета BNB в бандле — обёртка/развёртка WBNB командой WRAP_ETH / UNWRAP_WETH в
//! execute КАЖДОГО Universal Router бандла (exec/bundle_native.rs). Нога форка (свой classic V2 Router) обёртку не
//! делает — маршрут с BNB через ногу форка неисполним с причиной.
//! Решение 39 (проверка и в котировке графа — Тигран, 03.11): у монеты с налогом путь через промежуточную (нога из 2+
//! хопов) — только пулы V2 на каждом шаге (как exec/live.rs, exec/live_hops.rs); иначе маршрут неисполним (сигнал).
//! Решение 122, часть X: при заданном exec.executor_address маршрут, каждый шаг которого — вид исполнителя
//! (exec/executor/entry.rs::supported), исполним одной транзакцией через свой исполнитель — бандловые каналы не нужны,
//! нога может пересекать роутеры и семейства; без адреса — как раньше.
//! Кто вызывает: quote/graph_route.rs (executable), exec/trade.rs (graph_form).
//! От чего зависит: exec/executor/{mod, entry}.rs, pools/families.rs (Family, smart_ok, classify), send/mod.rs (planned_bundle_ok), quote/path.rs.
//! Настройки: exec.executor_address, dex.route_max_hops, dex.bundle_max_routers [bsc_dex], dex.router, dex.universal_router [DEX], channel.*.

use alloy_primitives::Address;

use crate::core::types::{DexRoute, PoolKind};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::exec::executor::{self, entry};
use super::super::pools::families::{self, Family, smart_ok};
use super::path::Path;

/// Исполним ли маршрут одной транзакцией или бандлом как есть (решение 84). Принимает: доли, нативная ли монета (BNB),
/// есть ли у монеты налог (решение 39). Возвращает: Ok — исполним; Err — причина (сигнал). Ошибок иных нет.
pub fn executable(ctx: &Ctx, legs: &[Path], native: bool, taxed: bool) -> Result<(), String> {
    // Пустой маршрут не исполним (защитный guard; split этого не даёт).
    if legs.is_empty() {
        return Err("пустой маршрут".to_string());
    }
    // R2: длина ноги — не больше dex.route_max_hops (перебор путей и так не строит длиннее; проверка явная).
    let max_hops = ctx.int("dex.route_max_hops").map_err(|e| e.message)?;
    if let Some(p) = legs.iter().find(|p| p.hops.len() as u64 > max_hops) {
        return Err(format!(
            "нога маршрута из {} хопов длиннее dex.route_max_hops ({max_hops}) — сигнал",
            p.hops.len()
        ));
    }
    // Решение 39: у монеты с налогом путь через промежуточную — только V2 на каждом шаге (сделка это тоже проверяет).
    if taxed
        && let Some(h) = legs
            .iter()
            .filter(|p| p.hops.len() > 1)
            .flat_map(|p| p.hops.iter())
            .find(|h| h.entry.pool.kind != PoolKind::V2)
    {
        return Err(format!(
            "у монеты налог — путь через промежуточную только V2 (решение 39), а шаг идёт через {} {:?} — сигнал",
            h.entry.family.title, h.entry.pool.kind
        ));
    }
    // Решение 122 (часть X): задан exec.executor_address и каждый шаг — вид исполнителя (отметка готовности семейства,
    // exec/executor/entry.rs::supported) — маршрут исполним одной транзакцией через исполнитель: бандловые каналы не
    // нужны, нога может пересекать роутеры и семейства. Нативная монета через исполнитель не идёт — как раньше.
    if !native && via_executor(ctx, legs)? {
        return Ok(());
    }
    // Защитно: хоп без своего пути исполнения (tradable=false) — сигнал, авто-сделкой не исполняется.
    if let Some(h) = legs
        .iter()
        .flat_map(|p| p.hops.iter())
        .find(|h| !h.entry.family.tradable)
    {
        return Err(format!(
            "маршрут проходит через {} без своего пути исполнения — это сигнал, авто-сделкой не исполняется",
            h.entry.family.id
        ));
    }
    // Форк исполняется ТОЛЬКО своим роутером бандлом (решения 59, 82) — не прямым/умным путём Universal Router.
    let has_fork = legs
        .iter()
        .flat_map(|p| p.hops.iter())
        .any(|h| h.entry.family.fork);
    if !has_fork {
        if legs.len() == 1 && legs[0].hops.len() == 1 {
            return Ok(());
        }
        if legs
            .iter()
            .all(|p| p.hops.iter().all(|h| smart_ok(ctx, h.entry.family)))
        {
            return Ok(());
        }
    }
    // R3: BNB обёртывается/развёртывается командой Universal Router; у роутера форка такой команды в бандле нет.
    if native && has_fork {
        return Err(
            "нативная монета BNB через ногу форка (свой classic V2 Router): обёртка WBNB в бандле делается только \
             командой Universal Router — маршрут неисполним (сигнал)"
                .to_string(),
        );
    }
    // Мульти-роутер или форк: исполним, если собирается в атомарный бандл свопов (решение 84, exec/bundle.rs).
    collectable(ctx, legs)
}

/// Пойдёт ли маршрут через исполнитель: адрес задан и все шаги — виды исполнителя. Ошибки (текст): адрес не читается.
fn via_executor(ctx: &Ctx, legs: &[Path]) -> Result<bool, String> {
    if executor::address(ctx).map_err(|e| e.message)?.is_none() {
        return Ok(false);
    }
    Ok(legs
        .iter()
        .flat_map(|p| p.hops.iter())
        .all(|h| entry::supported(h.entry.family, &h.entry.pool.reference).is_ok()))
}

/// Исполняющий роутер хопа: у форка — его собственный classic V2 Router (dex.router), иначе — Universal Router своей
/// фабрики (dex.universal_router). Так collectable и exec/bundle.rs считают роутеры одинаково. Ошибки: NotConfigured.
fn exec_router(ctx: &Ctx, f: &Family) -> GatewayResult<Address> {
    if f.fork {
        ctx.dex_addr("dex.router", f.id)
    } else {
        ctx.dex_addr("dex.universal_router", f.id)
    }
}

/// Собирается ли маршрут в бандл (решение 84): все включённые каналы отправки умеют бандл (решение 121 B3), ни одна
/// нога не пересекает роутеры (как exec/bundle::single_router/fork_router) и всего роутеров ≤ dex.bundle_max_routers.
/// Ошибка — причина несобираемости.
fn collectable(ctx: &Ctx, legs: &[Path]) -> Result<(), String> {
    super::super::send::planned_bundle_ok(ctx)?;
    // Не задано → ctx.int вернёт ошибку (настройка L::Positive, значение всегда ≥ 1, если задано).
    let max = ctx.int("dex.bundle_max_routers").map_err(|e| e.message)?;
    let mut routers: Vec<Address> = Vec::new();
    for p in legs {
        let mut leg_router: Option<Address> = None;
        for h in &p.hops {
            let r = exec_router(ctx, h.entry.family).map_err(|e| e.message)?;
            match leg_router {
                None => leg_router = Some(r),
                Some(x) if x != r => {
                    return Err("нога маршрута пересекает роутеры (UR/форк) — бандл такую ногу не собирает (сигнал)".into());
                }
                _ => {}
            }
        }
        if let Some(r) = leg_router
            && !routers.contains(&r)
        {
            routers.push(r);
        }
    }
    if routers.len() as u64 > max {
        return Err(format!(
            "маршрут задействует {} роутеров — больше dex.bundle_max_routers ({max}): бандлом не исполняется (сигнал)",
            routers.len()
        ));
    }
    Ok(())
}

/// Форма исполнимого графового маршрута для боевого пути (exec/trade.rs). Классификация по тем же правилам, что
/// executable: прямая (одна доля, один хоп), один Universal Router PancakeSwap (умный путь), иначе — бандл.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphForm {
    /// Прямая форма: один пул, один хоп — идёт прямым путём (exec/trade.rs).
    Direct,
    /// Всё через один Universal Router PancakeSwap — идёт умным путём (exec/trade_smart.rs).
    OneUr,
    /// Мульти-роутер — идёт бандлом свопов (exec/bundle_trade.rs).
    Bundle,
}

/// Форма исполнимого графового маршрута. Принимает: маршрут из котировки. Возвращает: форму. Ошибок нет
/// (вызывается только для уже исполнимого маршрута, DexRoute.executable = true).
pub fn graph_form(ctx: &Ctx, route: &DexRoute) -> GraphForm {
    // Форк исполняется только своим роутером бандлом (решения 59, 82): проверяем до прямой формы.
    let has_fork = route
        .legs
        .iter()
        .flat_map(|l| l.hops.iter())
        .any(|h| families::classify(ctx, &h.family).is_ok_and(|f| f.fork));
    if has_fork {
        return GraphForm::Bundle;
    }
    // Прямая форма — только у семейства со своим Universal Router (tradable, не форк); шаг другого семейства в
    // исполнимом маршруте возможен только через исполнитель (решение 122) — форма «бандл».
    let own_ur = |h: &crate::core::types::RouteHop| {
        families::classify(ctx, &h.family).is_ok_and(|f| f.tradable && !f.fork)
    };
    if route.legs.len() == 1
        && route
            .legs
            .first()
            .is_some_and(|l| l.hops.len() == 1 && l.hops.iter().all(own_ur))
    {
        return GraphForm::Direct;
    }
    let one_ur = route.legs.iter().all(|l| {
        l.hops
            .iter()
            .all(|h| families::classify(ctx, &h.family).is_ok_and(|f| smart_ok(ctx, f)))
    });
    if one_ur {
        GraphForm::OneUr
    } else {
        GraphForm::Bundle
    }
}
