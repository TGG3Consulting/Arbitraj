//! Файл: venues/bsc_dex/pools/graph_eval.rs
//! Что делает: решение 308 (задача t137) — оценка ОДНОГО пула для графа маршрутов, общая для накопительной карты
//! (graph_work.rs, круг владельца) и сборки с нуля (graph_full.rs): одна функция — одинаковый итог (инвариант И6).
//! Правила те же, что у pools/graph.rs build: в граф идёт достоверный, не спящий пул graphable-вида, у которого глубина
//! посчиталась (книга глубины — память по ключу, решения 73, 301, 307) и хотя бы одна сторона глубокая; ранг —
//! max(глубина покупки, продажи). Ребро (копия записи на момент оценки) создаётся здесь же — при каждом изменении пула
//! оценка повторяется, поэтому копия в снимке — на версии снимка (как у build). Цену расчёт берёт не из копии, а свежей из
//! реестра (route_cache::fresh — гидратация).
//! Кто вызывает: graph_work.rs, graph_full.rs, graph_round.rs.
//! От чего зависит: graph.rs (Edge, pair_key), depth_book.rs, depth_key.rs (Prep), registry.rs, tokens.rs, ctx.rs.
//! Настройки: через книгу глубины — dex.min_pool_liquidity_usd, dex.depth_band_pct, dex.depth_hysteresis_pct [bsc_dex].

use std::collections::HashSet;
use std::sync::Arc;

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
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
    e: &Entry,
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
    if !(d.deep_buy || d.deep_sell) {
        return rec;
    }
    let edge = Edge {
        entry: e.clone(),
        a: e.coin,
        b: e.usdt,
    };
    rec.cand = Some((d.band.buy.max(d.band.sell), Arc::new(edge)));
    rec
}

/// Оценка набора пулов по текущему реестру: пул есть — Some(Rec), пропал — None. Принимает: как eval, набор ссылок.
/// Возвращает: пары (ссылка, оценка). Ошибки (текст): настройки глубины не заданы или неверны (Book::prep).
pub fn evaluate(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
    refs: &HashSet<PoolRef>,
) -> Result<Vec<(PoolRef, Option<Rec>)>, String> {
    let prep = Book::prep(ctx, tokens)?;
    Ok(refs
        .iter()
        .map(|r| {
            let rec = reg
                .entry(r)
                .map(|e| eval(ctx, reg, tokens, usdt_dec, &prep, &e));
            (r.clone(), rec)
        })
        .collect())
}
