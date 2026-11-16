//! Файл: venues/bsc_dex/quote/graph_reuse.rs
//! Что делает: решение 336 (задача t157, п. 2б) — то, что в котировке графа (quote/graph_route.rs) от объёма НЕ
//! зависит, считается один раз на подбор объёма кандидата, а не на каждой из десятков–сотен точек:
//! набор путей (кэш путей / снимок графа / перебор — quote/route_cache.rs), decimals токенов путей и допуск каждого хопа
//! (quote/graph_legs.rs → select.rs: достоверность, крючок, квотер, газ, налог пула, глубина). От объёма зависят и
//! считаются на каждой точке, как раньше: газ пути в USDT (цена газа точки), отбор лучших путей, сплит, исполнимость,
//! затраты, классика и выбор между ними.
//! Где хранится: в коробе подбора ядра (gateway/calc_share.rs — один на подбор объёма, переносится в поток счёта
//! quote/cpu.rs → cpu_local.rs). Финальная котировка и всё вне подбора идут без короба — полностью заново.
//! Свежесть: на каждой точке у каждого пути сверяется поколение состояния каждого его пула (Entry.state_gen,
//! pools/state_gen.rs), достоверность и «спит» с реестром; совпало у всех пулов пути — берутся ноги из короба (то же
//! состояние — тот же расчёт); не совпало — путь берётся свежим из реестра (route_cache::hydrate) и допуск считается
//! заново (пул пропал / недостоверен / спит — путь выпадает, как у кэша путей решения 189). Поколение 0 (запись вне
//! реестра) — всегда заново.
//! Что в пределах одного подбора НЕ видно (видно со следующего кандидата и в финальной котировке): новый путь (новый
//! пул, пул вырос выше порога графа), смена проверок пула без смены его состояния (квотер, налог, крючок, газ),
//! смена настроек допуска и курса встречного токена для глубины.
//! Ключ в коробе — всё, от чего зависит набор и допуск: концы, сторона, пределы перебора, промежуточные, подготовка
//! глубины (dex.quote_prep_once), нативная монета, плановая цена газа (пулы Solidly), снимок рынка.
//! dex.quote_reuse не задана или нет — как до 336 (всё на каждой точке).
//! Кто вызывает: quote/graph_route.rs.
//! От чего зависит: quote/graph_legs.rs, quote/route_cache.rs, quote/cpu_local.rs, gateway/calc_share.rs,
//! pools/depth_book.rs (Book::prep), pools/registry.rs, quote/quote_timing.rs.
//! Настройки: dex.quote_reuse [bsc_dex] (необязательная).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use alloy_primitives::Address;

use crate::gateway::calc_share::Share;

use super::super::pools::depth_book::Book;
use super::super::pools::registry::Registry;
use super::quote_timing::{Stage, stage};
use super::route_cache::{self, Search};
use super::route_calc::Leg;
use super::route_paths::RoutePath;
use super::via::Via;
use super::{cpu_local, graph_legs};

/// Допуск путей одного подбора: путь (записи пулов — на момент допуска) и его ноги или причина недопуска; decimals.
pub struct Admitted {
    list: Vec<(RoutePath, Result<Vec<Leg>, String>)>,
    dec: HashMap<Address, u8>,
}

/// Место в коробе подбора для этой котировки. Принимает: расчёт путей, что перебирать, нативная ли монета.
/// Возвращает: (короб, ключ); None — dex.quote_reuse не «да» или котировка вне подбора объёма. Ошибок нет.
pub fn slot(v: &Via<'_>, s: &Search<'_>, native: bool) -> Option<(Arc<Share>, String)> {
    let ctx = &v.q.ctx;
    if !ctx.flag("dex.quote_reuse").unwrap_or(false) {
        return None;
    }
    let share = cpu_local::share()?;
    let mut mids: Vec<String> = s.allowed.iter().map(|a| a.to_string()).collect();
    mids.sort();
    let per_edge = ctx
        .int("dex.route_max_pools_per_edge")
        .map(|v| v.to_string())
        .unwrap_or_default();
    let key = format!(
        "bsc_dex.graph_reuse|{}|{}|{:?}|{}|{}|{:?}|{per_edge}|{}|{}|{native}|{:?}|{:?}|{}",
        s.usdt,
        s.coin,
        v.side,
        s.max_hops,
        s.max_paths,
        s.cap,
        mids.join(","),
        graph_legs::prep_once(ctx),
        v.q.plan_gas,
        v.q.frozen_head,
        v.usdt_dec,
    );
    Some((share, key))
}

/// Допуск путей с нуля (как было в quote/graph_route.rs): одна подготовка глубины на котировку (решение 311 А), путь
/// во владение (311 В). Принимает: расчёт путей, пути, decimals, замер, хранить ли для короба. Возвращает: ноги или
/// причину по каждому пути (порядок путей) и, если просили, допуск для короба. Ошибок нет.
pub fn admit(
    v: &Via<'_>,
    paths: Vec<RoutePath>,
    dec: HashMap<Address, u8>,
    on: bool,
    keep: bool,
) -> (Vec<Result<Vec<Leg>, String>>, Option<Admitted>) {
    let ctx = &v.q.ctx;
    // Подготовка (RefCell внутри) живёт только в этой функции без await.
    let prep = if graph_legs::prep_once(ctx) {
        stage(Stage::Prep, on, || Book::prep(ctx, &v.q.tokens).ok())
    } else {
        None
    };
    let mut out = Vec::with_capacity(paths.len());
    let mut kept = Vec::new();
    for p in paths {
        let copy = keep.then(|| p.clone());
        let legs = stage(Stage::Legs, on, || {
            graph_legs::legs_prep(v.q, p, v.side, v.usdt_dec, &dec, prep.as_ref())
        });
        if let Some(c) = copy {
            kept.push((c, legs.clone()));
        }
        out.push(legs);
    }
    let adm = keep.then_some(Admitted { list: kept, dec });
    (out, adm)
}

/// Допуск путей по коробу (см. шапку «Свежесть»). Принимает: расчёт путей, допуск из короба, замер. Возвращает: ноги
/// или причину по каждому годному пути (порядок путей). Ошибок нет.
pub fn refresh(v: &Via<'_>, a: &Admitted, on: bool) -> Vec<Result<Vec<Leg>, String>> {
    let ctx = &v.q.ctx;
    let reg = &v.q.reg;
    let mut prep = None;
    let mut out = Vec::with_capacity(a.list.len());
    for (path, res) in &a.list {
        if same(reg, path) {
            out.push(res.clone());
            continue;
        }
        let Some(p) = route_cache::hydrate(reg, std::slice::from_ref(path)).pop() else {
            continue;
        };
        if prep.is_none() && graph_legs::prep_once(ctx) {
            prep = stage(Stage::Prep, on, || Book::prep(ctx, &v.q.tokens).ok());
        }
        let legs = stage(Stage::Legs, on, || {
            graph_legs::legs_prep(v.q, p, v.side, v.usdt_dec, &a.dec, prep.as_ref())
        });
        out.push(legs);
    }
    out
}

/// Состояние всех пулов пути то же, что при допуске: поколение (не 0), достоверность, «спит». Ошибок нет.
fn same(reg: &Registry, path: &RoutePath) -> bool {
    let mut seen = HashSet::new();
    path.steps.iter().all(|s| {
        let e = &s.edge.entry;
        if !seen.insert(e.pool.reference.clone()) {
            return true;
        }
        e.state_gen != 0
            && reg.entry(&e.pool.reference).is_some_and(|x| {
                x.state_gen == e.state_gen && x.valid == e.valid && x.asleep == e.asleep
            })
    })
}
