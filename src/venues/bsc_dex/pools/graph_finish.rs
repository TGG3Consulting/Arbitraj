//! Файл: venues/bsc_dex/pools/graph_finish.rs
//! Что делает: решение 308 (задача t137) — окончание полной сборки карты графа у фонового владельца (graph_owner.rs):
//! сборка упала, отменена или не удалась — Error в журнал (кроме отмены), действующий снимок остаётся (торговля по графу
//! не останавливается); собрана на других настройках (сменились во время сборки) — выброшена, запрос новой; иначе —
//! доводка до текущей версии реестра и сверка с накопительной картой (graph_full.rs reconcile) в потоке блокирующих
//! задач, публикация снимка, собранного с нуля (самоисцеление — при расхождении и без него), событие dex.graph_mismatch
//! при расхождении, счётчики экрана и замер dex.graph_build (вид full: сборка и доводка — отдельно, ревью №2 C), срок
//! следующей самопроверки dex.graph_verify_s; запрос пересборки, пришедший во время сборки, — сразу после неё (ревью №2 E).
//! Ревью №2 (A, B): на время доводки ставится признак «догоняющая сборка» — читатели берут действующий снимок, отставшим
//! он не считается; доводка не помечается пульсом круга — сторож зависания её не снимает.
//! Кто вызывает: graph_owner.rs (run).
//! От чего зависит: graph_owner.rs (Owner, St, FullResult), graph_step.rs (basis_now), graph_full.rs, graph_eval.rs
//! (CANCELLED), graph_snap.rs, graph_alarm.rs, graph_stats.rs, graph_timing.rs, registry.rs.
//! Настройки: dex.graph_verify_s [bsc_dex] (необязательная).

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;

use crate::core::types::DexGraphFull;

use super::graph_eval::CANCELLED;
use super::graph_full;
use super::graph_owner::{FullResult, Owner, St};
use super::graph_snap::{Bg, Snapshot};
use super::graph_timing::{self, BgMeasure};
use super::{graph_alarm, graph_step};

/// Миллисекунды с момента `t`. Ошибок нет.
fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Неудачная сборка: счётчики экрана и (кроме отмены) Error в журнал (без повторов — graph_alarm.rs) и пауза до
/// следующей попытки dex.graph_retry_pause_s (ревью №1 O). Ошибок нет.
fn failed(o: &Owner, bg: &Bg, st: &mut St, reason: &str, build_ms: f64, why: String) {
    if why != CANCELLED {
        graph_alarm::full_failed(&o.ctx, bg, reason, &why);
        st.retry_at = o
            .ctx
            .int("dex.graph_retry_pause_s")
            .ok()
            .map(|s| tokio::time::Instant::now() + Duration::from_secs(s));
    }
    bg.stats.full(DexGraphFull {
        at: Utc::now(),
        reason: reason.to_string(),
        ms: build_ms,
        build_ms,
        reconcile_ms: 0.0,
        pools: 0,
        edges: 0,
        compared: false,
        differences: 0,
        outcome: why,
    });
}

/// Окончание полной сборки (см. шапку). Принимает: окружение, состояние, итог потока сборки. Ошибки (текст): поток
/// доводки упал (паника) — владелец выходит, сторож перезапустит.
pub(super) async fn finish(
    o: &Owner,
    bg: &Arc<Bg>,
    st: &mut St,
    r: FullResult,
) -> Result<(), String> {
    let Some(job) = st.full.take() else {
        return Ok(());
    };
    // Ревью №2 (E): запрос, пришедший во время сборки, — следующей сборкой (step заберёт его на этом же пробуждении).
    if let Some(again) = st.again.take() {
        bg.request_full(again);
    }
    let ctx = &o.ctx;
    let build_ms = ms(job.started);
    let (full, pools) = match r {
        Ok(Ok(x)) => x,
        Ok(Err(why)) => {
            failed(o, bg, st, &job.reason, build_ms, why);
            return Ok(());
        }
        Err(e) => {
            failed(
                o,
                bg,
                st,
                &job.reason,
                build_ms,
                format!("поток сборки упал: {e}"),
            );
            return Ok(());
        }
    };
    // Настройки не готовы или сменились за время сборки — итог не годится; step разберётся на этом же пробуждении.
    let Ok((basis, per_edge)) = graph_step::basis_now(o).await else {
        return Ok(());
    };
    if full.basis != basis {
        bg.request_full("настройки графа сменились во время полной пересборки");
        return Ok(());
    }
    let inc = st.work.take().filter(|w| w.basis == basis);
    let v0 = o.reg.version();
    let (drained, oldest) = bg.changes.drain();
    let mut refs = job.since;
    refs.extend(drained);
    let (c, reg, t) = (ctx.clone(), o.reg.clone(), o.tokens.clone());
    let (b2, generation) = (bg.clone(), o.generation);
    let t1 = Instant::now();
    bg.set_catching_up(true);
    let res = tokio::task::spawn_blocking(move || {
        let alive = || b2.is_current(generation);
        graph_full::reconcile(&c, &reg, &t, inc, full, refs, per_edge, &alive)
    })
    .await;
    bg.set_catching_up(false);
    let reconcile_ms = ms(t1);
    let rec = match res.map_err(|e| format!("доводка полной пересборки графа упала: {e}"))?
    {
        Ok(x) => x,
        Err(e) => {
            let (inc, refs, why) = *e;
            st.work = inc;
            // Ревью №1 (N): забранные пулы не применены — обратно в набор, следующий круг их повторит.
            bg.changes.remark(refs);
            failed(o, bg, st, &job.reason, build_ms, why);
            return Ok(());
        }
    };
    let edges = rec.graph.len();
    let snap = Snapshot {
        graph: Arc::new(rec.graph),
        basis,
        version: v0,
        seq: bg.next_seq(),
        full: true,
        at: Instant::now(),
        at_utc: Utc::now(),
    };
    st.work = Some(rec.work);
    if !bg.publish(o.generation, snap) {
        return Ok(());
    }
    st.verify_s = ctx.int("dex.graph_verify_s").ok();
    st.next_verify = st
        .verify_s
        .map(|s| tokio::time::Instant::now() + Duration::from_secs(s));
    let diffs = rec.diff.as_ref().map(Vec::len);
    let took = ms(job.started);
    bg.stats.full(DexGraphFull {
        at: Utc::now(),
        reason: job.reason.clone(),
        ms: took,
        build_ms,
        reconcile_ms,
        pools,
        edges,
        compared: diffs.is_some(),
        differences: diffs.unwrap_or(0),
        outcome: "ok".into(),
    });
    let lag_ms = oldest.map_or(0, |t| {
        u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
    });
    graph_timing::report_bg(
        ctx,
        &BgMeasure {
            kind: "full",
            reason: Some(job.reason.clone()),
            ms: took,
            build_ms: Some(build_ms),
            reconcile_ms: Some(reconcile_ms),
            evaluated: rec.evaluated,
            changed: None,
            pairs: 0,
            edges,
            lag_ms,
            pending: bg.changes.pending(),
            pools: Some(pools),
            differences: diffs,
        },
    );
    if let Some(d) = rec.diff.as_ref()
        && !d.is_empty()
    {
        graph_alarm::mismatch(ctx, bg, &job.reason, d);
    }
    Ok(())
}
