//! Файл: venues/bsc_dex/pools/graph_step.rs
//! Что делает: решение 308 (задача t137) — одно пробуждение фонового владельца графа (graph_owner.rs run):
//! режим dex.graph_background выключен — снимок снят, слежение за изменениями выключено; настройки графа/глубины или USDT
//! не готовы — снимка нет с причиной (graph_alarm.rs, экран); нет карты, сменились настройки графа или пришёл запрос —
//! запуск полной сборки (graph_full.rs build) в потоке блокирующих задач, параллельно кругам; есть изменённые пулы и
//! карта на текущих настройках — накопительный круг (graph_round.rs) в потоке блокирующих задач, публикация снимка
//! (только текущего поколения), счётчики экрана (graph_stats.rs) и замер dex.graph_build (graph_timing.rs).
//! Карта на прежних настройках (идёт полная сборка после их смены) накопительно не обновляется — читатели берут
//! прежний снимок (ступень Б, graph_read.rs), изменения копятся в наборе и доводятся в graph_finish.rs.
//! Кто вызывает: graph_owner.rs (run), graph_finish.rs (basis_now).
//! От чего зависит: graph_owner.rs (Owner, St, FullJob), graph_snap.rs, graph_round.rs, graph_full.rs, graph_alarm.rs,
//! graph_stats.rs, graph_timing.rs, graph_read.rs, graph_cache.rs (graph_settings), depth_book.rs, tokens.rs, ctx.rs.
//! Настройки: dex.graph_background, dex.graph_verify_s, dex.route_max_pools_per_edge [bsc_dex]; через книгу глубины —
//! dex.depth_band_pct, dex.min_pool_liquidity_usd, dex.depth_hysteresis_pct; venue.quote_asset (USDT).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;

use crate::core::types::DexGraphRound;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::depth_book::{self, Book};
use super::graph_cache::graph_settings;
use super::graph_owner::{FullJob, Owner, St};
use super::graph_snap::{Basis, Bg, Snapshot};
use super::graph_timing::{self, BgMeasure};
use super::{graph_alarm, graph_read, graph_round};

/// Одно пробуждение (см. шапку). Ошибки (текст): круг упал (паника) — владелец выходит, сторож перезапустит.
pub(super) async fn step(o: &Owner, bg: &Arc<Bg>, st: &mut St) -> Result<(), String> {
    let ctx = &o.ctx;
    if !graph_read::background(ctx) {
        if bg.changes.tracking() || bg.load().is_some() || st.work.is_some() {
            bg.changes.track(false);
            bg.clear(None);
            st.work = None;
            st.full = None;
            st.next_verify = None;
        }
        let _ = bg.take_full();
        return Ok(());
    }
    bg.changes.track(true);
    let (basis, per_edge) = match basis_now(o).await {
        Ok(b) => {
            graph_alarm::not_built(ctx, bg, None);
            b
        }
        Err(why) => {
            graph_alarm::not_built(ctx, bg, Some(&why));
            bg.clear(Some(why));
            st.work = None;
            return Ok(());
        }
    };
    if st.next_verify.is_none()
        && st.work.is_some()
        && let Ok(s) = ctx.int("dex.graph_verify_s")
    {
        st.next_verify = Some(tokio::time::Instant::now() + Duration::from_secs(s));
    }
    let req = bg.take_full();
    let need = st.work.as_ref().is_none_or(|w| w.basis != basis);
    let running = st.full.as_ref().is_some_and(|j| j.basis == basis);
    if (need || req.is_some()) && !running {
        let reason = req.unwrap_or_else(|| match st.work {
            None => "снимка нет — первая сборка".to_string(),
            Some(_) => "сменились настройки графа".to_string(),
        });
        start_full(o, st, basis.clone(), reason);
    }
    round(o, bg, st, basis, per_edge).await
}

/// Настройки снимка сейчас. Возвращает: basis и dex.route_max_pools_per_edge. Ошибки (текст): не задана настройка
/// графа/глубины, USDT не известен, decimals USDT не прочитаны.
pub(super) async fn basis_now(o: &Owner) -> Result<(Basis, usize), String> {
    let per_edge = checks(&o.ctx, &o.tokens)?;
    let usdt = o.tokens.usdt(&o.ctx).map_err(|e| e.message)?;
    let usdt_dec = o
        .tokens
        .decimals(&o.ctx, usdt)
        .await
        .map_err(|e| e.message)?;
    let basis = Basis {
        settings: graph_settings(&o.ctx),
        usdt,
        usdt_dec,
    };
    Ok((basis, per_edge))
}

/// Синхронные проверки настроек (как у build): глубина, dex.route_max_pools_per_edge > 0, гистерезис. Ошибки (текст).
fn checks(ctx: &Ctx, tokens: &Tokens) -> Result<usize, String> {
    if let Some(name) = depth_book::missing_setting(ctx) {
        return Err(format!("не задана {name}"));
    }
    let n = ctx
        .int("dex.route_max_pools_per_edge")
        .map_err(|e| e.message)?;
    if n == 0 {
        return Err("dex.route_max_pools_per_edge должна быть больше нуля".into());
    }
    Book::prep(ctx, tokens).map(|_| usize::try_from(n).unwrap_or(usize::MAX))
}

/// Запуск полной сборки в потоке блокирующих задач. Карты ещё нет — набор изменений сбрасывается (сборка прочитает
/// реестр позже; изменения после — в наборе, их доведёт finish). Ошибок нет.
fn start_full(o: &Owner, st: &mut St, basis: Basis, reason: String) {
    let bg = o.reg.graph_cache().bg();
    if st.work.is_none() {
        let _ = bg.changes.drain();
    }
    let (ctx, reg, tokens, b) = (
        o.ctx.clone(),
        o.reg.clone(),
        o.tokens.clone(),
        basis.clone(),
    );
    let handle =
        tokio::task::spawn_blocking(move || super::graph_full::build(&ctx, &reg, &tokens, b));
    st.full = Some(FullJob {
        handle,
        since: HashSet::new(),
        reason,
        started: Instant::now(),
        basis,
    });
}

/// Накопительный круг, если есть изменения и карта на текущих настройках (на прежних — ждём полную сборку, снимок
/// прежний — ступень Б). Ошибки (текст): круг упал (паника).
async fn round(
    o: &Owner,
    bg: &Arc<Bg>,
    st: &mut St,
    basis: Basis,
    per_edge: usize,
) -> Result<(), String> {
    let Some(w) = st.work.take() else {
        return Ok(());
    };
    if w.basis != basis || bg.changes.pending() == 0 {
        st.work = Some(w);
        return Ok(());
    }
    let v0 = o.reg.version();
    let (refs, oldest) = bg.changes.drain();
    let (ctx, reg, tokens) = (o.ctx.clone(), o.reg.clone(), o.tokens.clone());
    let t0 = Instant::now();
    bg.round_begin();
    let res = tokio::task::spawn_blocking(move || {
        let mut w = w;
        let out = graph_round::round(&ctx, &reg, &tokens, &mut w, refs, per_edge);
        (w, out)
    })
    .await;
    bg.round_end();
    let (w, out) = res.map_err(|e| format!("накопительный круг графа упал: {e}"))?;
    st.work = Some(w);
    let out = match out {
        Ok(x) => x,
        Err(why) => {
            // Забранные изменения к карте не применены — их покроет полная пересборка (прочитает реестр заново).
            graph_alarm::not_built(&o.ctx, bg, Some(&why));
            bg.request_full(format!("накопительный круг не прошёл: {why}"));
            return Ok(());
        }
    };
    if let Some(j) = st.full.as_mut() {
        j.since.extend(out.evaluated.iter().cloned());
    }
    let edges = out.graph.len();
    let snap = Snapshot {
        graph: Arc::new(out.graph),
        basis,
        version: v0,
        seq: bg.next_seq(),
        full: false,
        at: Instant::now(),
        at_utc: Utc::now(),
    };
    if !bg.publish(o.generation, snap) {
        return Ok(());
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let lag_ms = oldest.map_or(0, |t| {
        u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
    });
    let pending = bg.changes.pending();
    bg.stats.round(DexGraphRound {
        at: Utc::now(),
        ms,
        evaluated: out.evaluated.len(),
        pairs: out.pairs,
        edges,
        lag_ms,
        pending_after: pending,
    });
    graph_timing::report_bg(
        &o.ctx,
        &BgMeasure {
            kind: "incr",
            reason: None,
            ms,
            evaluated: out.evaluated.len(),
            pairs: out.pairs,
            edges,
            lag_ms,
            pending,
            pools: None,
            differences: None,
        },
    );
    Ok(())
}
