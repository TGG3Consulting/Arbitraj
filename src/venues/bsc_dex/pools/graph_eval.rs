//! Файл: venues/bsc_dex/pools/graph_eval.rs
//! Что делает: решение 308 (задача t137) — оценка ОДНОГО пула для графа маршрутов, общая для накопительной карты
//! (graph_work.rs, круг владельца) и сборки с нуля (graph_full.rs): одна функция — одинаковый итог (инвариант И6).
//! Правила те же, что у pools/graph.rs build: в граф идёт достоверный, не спящий пул graphable-вида, у которого глубина
//! посчиталась (книга глубины — память по ключу, решения 73, 301, 307) и хотя бы одна сторона глубокая; ранг —
//! max(глубина покупки, продажи). Ребро (копия записи на момент оценки) создаётся здесь же — при каждом изменении пула
//! оценка повторяется, поэтому копия в снимке — на версии снимка (как у build). Цену расчёт берёт не из копии, а свежей из
//! реестра (route_cache::fresh — гидратация).
//! Решение 336 (задача t157): evaluate при dex.graph_threads > 1 — на нескольких потоках (quote/par.rs), порядок итогов тот же.
//! Кто вызывает: graph_work.rs, graph_full.rs, graph_round.rs.
//! От чего зависит: graph.rs (Edge, pair_key), quote/side_fit.rs (мелкий пул, решение 393), depth_book.rs, depth_key.rs (Prep), registry.rs, tokens.rs, ctx.rs.
//! Настройки: dex.graph_threads (решение 336); через книгу глубины — dex.min_pool_liquidity_usd, dex.depth_band_pct, dex.depth_hysteresis_pct [bsc_dex].

use std::collections::HashSet;
use std::sync::Arc;

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::quote::{par, shallow_pick};
use super::super::tokens::Tokens;
use super::depth_book::Book;
use super::depth_key::Prep;
use super::graph::{Edge, pair_key};
use super::registry::{Entry, Registry};

/// Пул в карте графа: пара, монета и встречный токен (для зависимостей по курсу), и — если он кандидат ребра — ранг
/// и ребро.
#[derive(Clone)]
pub struct Rec {
    pub pair: (Address, Address),
    pub coin: Address,
    pub counter: Address,
    pub cand: Option<(Decimal, Arc<Edge>)>,
}

/// Оценка пула. Принимает: окружение, реестр, токены, знаки USDT, подготовку глубины, запись. Возвращает: Rec
/// (cand = None — в граф не идёт). Ошибок нет: глубина не посчиталась — не кандидат (как build).
pub fn eval(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
    prep: &Prep,
    e: &Arc<Entry>,
) -> Rec {
    let mut rec = Rec {
        pair: pair_key(e.coin, e.usdt),
        coin: e.coin,
        counter: e.usdt,
        cand: None,
    };
    if !e.valid || e.asleep || !e.family.graphable {
        return rec;
    }
    let Ok(d) = reg.depth().depth_prep(ctx, reg, tokens, e, usdt_dec, prep) else {
        return rec;
    };
    // Решение 393: мелкий пул — кандидат, если хотя бы одну сторону пропускает сумма пробы (как pools/graph.rs build).
    // Решение 403: допуск — из опубликованного снимка отбора (quote/shallow_memo.rs), без замка и чтения настроек.
    if !(d.deep_buy || d.deep_sell) && !shallow_pick::admitted_snap(reg, e) {
        return rec;
    }
    // Решение 409: ребро держит тот же Arc записи (без копии).
    let edge = Edge {
        entry: Arc::clone(e),
        a: e.coin,
        b: e.usdt,
    };
    rec.cand = Some((d.band.buy.max(d.band.sell), Arc::new(edge)));
    rec
}

/// Оценка набора пулов по текущему реестру: пул есть — Some(Rec), пропал — None. Принимает: как eval, набор ссылок,
/// `alive` — жива ли работа (ревью №2 B: брошенная сторожем или заменённая работа останавливается на ближайшем пуле).
/// Возвращает: пары (ссылка, оценка). Ошибки (текст): настройки глубины не заданы или неверны (Book::prep); работа
/// отменена (CANCELLED).
pub fn evaluate(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
    refs: &HashSet<PoolRef>,
    alive: &dyn Fn() -> bool,
) -> Result<Vec<(PoolRef, Option<Rec>)>, String> {
    let prep = Book::prep(ctx, tokens)?;
    // Решение 336: при dex.graph_threads > 1 — на нескольких потоках (своя подготовка у потока), порядок итогов — порядок
    // набора, как ниже.
    let want = par::want(ctx, "dex.graph_threads");
    if want > 1 && refs.len() > 1 {
        let list: Vec<&PoolRef> = refs.iter().collect();
        let init = || Prep::new(ctx, tokens, prep.band, prep.min, prep.hyst);
        let one = |p: &mut Prep, r: &&PoolRef| {
            let rec = reg
                .entry(r)
                .map(|e| eval(ctx, reg, tokens, usdt_dec, p, &e));
            ((*r).clone(), rec)
        };
        return par::map_init(ctx, want, &list, init, one, alive)
            .ok_or_else(|| CANCELLED.to_string());
    }
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        if !alive() {
            return Err(CANCELLED.to_string());
        }
        let rec = reg
            .entry(r)
            .map(|e| eval(ctx, reg, tokens, usdt_dec, &prep, &e));
        out.push((r.clone(), rec));
    }
    Ok(out)
}

/// Текст ошибки «работа отменена» (сторож перезапустил владельца или сборка заменена новой).
pub const CANCELLED: &str = "работа графа отменена (новое поколение владельца или новая сборка)";
