//! Файл: venues/bsc_dex/pools/graph_finish.rs
//! Что делает: решение 308 (задача t137) — окончание полной сборки карты графа у фонового владельца (graph_owner.rs):
//! сборка упала или не удалась — Error в журнал, действующий снимок остаётся (торговля по графу не останавливается);
//! собрана на других настройках (сменились во время сборки) — выброшена, запрос новой; иначе — доводка до текущей версии
//! реестра и сверка с накопительной картой (graph_full.rs reconcile) в потоке блокирующих задач, публикация снимка,
//! собранного с нуля (самоисцеление — при расхождении и без него), событие dex.graph_mismatch при расхождении, счётчики
//! экрана и замер dex.graph_build (вид full), срок следующей самопроверки dex.graph_verify_s.
//! Кто вызывает: graph_owner.rs (run).
//! От чего зависит: graph_owner.rs (Owner, St, FullResult), graph_step.rs (basis_now), graph_full.rs, graph_snap.rs,
//! graph_alarm.rs, graph_stats.rs, graph_timing.rs, registry.rs.
//! Настройки: dex.graph_verify_s [bsc_dex] (необязательная).

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;

use crate::core::types::DexGraphFull;

use super::graph_full;
use super::graph_owner::{FullResult, Owner, St};
use super::graph_snap::{Bg, Snapshot};
use super::graph_timing::{self, BgMeasure};
use super::{graph_alarm, graph_step};

/// Запись итога полной пересборки в счётчики экрана. Ошибок нет.
fn note(
    bg: &Bg,
    reason: &str,
    ms: f64,
    pools: usize,
    edges: usize,
    diff: Option<usize>,
    outcome: String,
) {
    bg.stats.full(DexGraphFull {
        at: Utc::now(),
        reason: reason.to_string(),
        ms,
        pools,
        edges,
        compared: diff.is_some(),
        differences: diff.unwrap_or(0),
        outcome,
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
    let ctx = &o.ctx;
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let (full, pools) = match r {
        Ok(Ok(x)) => x,
        Ok(Err(why)) => {
            graph_alarm::full_failed(ctx, bg, &job.reason, &why);
            note(bg, &job.reason, ms(job.started), 0, 0, None, why);
            return Ok(());
        }
        Err(e) => {
            let why = format!("поток сборки упал: {e}");
            graph_alarm::full_failed(ctx, bg, &job.reason, &why);
            note(bg, &job.reason, ms(job.started), 0, 0, None, why);
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
    bg.round_begin();
    let res = tokio::task::spawn_blocking(move || {
        graph_full::reconcile(&c, &reg, &t, inc, full, refs, per_edge)
    })
    .await;
    bg.round_end();
    let rec = match res.map_err(|e| format!("доводка полной пересборки графа упала: {e}"))?
    {
        Ok(x) => x,
        Err(e) => {
            let (inc, why) = *e;
            st.work = inc;
            graph_alarm::full_failed(ctx, bg, &job.reason, &why);
            note(bg, &job.reason, ms(job.started), pools, 0, None, why);
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
    st.next_verify = ctx
        .int("dex.graph_verify_s")
        .ok()
        .map(|s| tokio::time::Instant::now() + Duration::from_secs(s));
    let diffs = rec.diff.as_ref().map(Vec::len);
    let took = ms(job.started);
    note(bg, &job.reason, took, pools, edges, diffs, "ok".into());
    let lag_ms = oldest.map_or(0, |t| {
        u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
    });
    graph_timing::report_bg(
        ctx,
        &BgMeasure {
            kind: "full",
            reason: Some(job.reason.clone()),
            ms: took,
            evaluated: rec.evaluated,
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
