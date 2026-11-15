//! Файл: venues/bsc_dex/pools/graph_publish.rs
//! Что делает: решение 308 (задача t137) — накопительный круг фонового владельца графа и публикация снимка:
//! есть изменённые пулы и карта — круг (graph_round.rs) в потоке блокирующих задач, публикация (только текущего
//! поколения), счётчики экрана (graph_stats.rs) и замер dex.graph_build (graph_timing.rs).
//! Ревью №2 (A): круги идут и на карте прежних настроек графа, пока после их смены строится новая (кроме смены USDT —
//! там другие концы путей): снимок не отстаёт, цена — по свежему состоянию (гидратация); снимок помечен прежними
//! настройками — читатели видят ступень Б, сверку с картой новых настроек finish не делает.
//! Ревью №2 (E), №1 (N): круг не прошёл — забранные пулы возвращаются в набор изменений (их повторит следующий круг или
//! доводка идущей полной сборки; ничего не теряется). Пульс круга (round_begin/end) — только здесь: его смотрит сторож зависания.
//! Кто вызывает: graph_step.rs (step).
//! От чего зависит: graph_owner.rs (Owner, St), graph_round.rs, graph_snap.rs, graph_alarm.rs, graph_eval.rs (CANCELLED),
//! graph_stats.rs, graph_timing.rs.
//! Настройки: dex.route_max_pools_per_edge (значение даёт вызывающий).

use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;

use crate::core::types::DexGraphRound;

use super::graph_eval::CANCELLED;
use super::graph_owner::{Owner, St};
use super::graph_snap::{Basis, Bg, Snapshot};
use super::graph_timing::{self, BgMeasure};
use super::{graph_alarm, graph_round};

/// Накопительный круг, если есть изменения и карта для того же USDT. Принимает: окружение, состояние, текущий basis,
/// dex.route_max_pools_per_edge. Ошибки (текст): круг упал (паника) — владелец выходит, сторож перезапустит.
pub(super) async fn round(
    o: &Owner,
    bg: &Arc<Bg>,
    st: &mut St,
    basis: Basis,
    per_edge: usize,
) -> Result<(), String> {
    let Some(w) = st.work.take() else {
        return Ok(());
    };
    let same_usdt = w.basis.usdt == basis.usdt && w.basis.usdt_dec == basis.usdt_dec;
    if !same_usdt || bg.changes.pending() == 0 {
        st.work = Some(w);
        return Ok(());
    }
    let v0 = o.reg.version();
    let (refs, oldest) = bg.changes.drain();
    let (ctx, reg, tokens) = (o.ctx.clone(), o.reg.clone(), o.tokens.clone());
    let (b2, generation) = (bg.clone(), o.generation);
    let t0 = Instant::now();
    bg.round_begin();
    let res = tokio::task::spawn_blocking(move || {
        let mut w = w;
        let alive = || b2.is_current(generation);
        let out = graph_round::round(&ctx, &reg, &tokens, &mut w, refs, per_edge, &alive);
        (w, out)
    })
    .await;
    bg.round_end();
    let (w, out) = res.map_err(|e| format!("накопительный круг графа упал: {e}"))?;
    let snap_basis = w.basis.clone();
    st.work = Some(w);
    let out = match out {
        Ok(x) => x,
        Err((refs, why)) => {
            // Ревью №1 (N), №2 (E): пулы не применены к карте — обратно в набор: следующий круг (или доводка идущей
            // сборки — она забирает набор) их повторит; ничего не теряется.
            bg.changes.remark(refs);
            if why != CANCELLED {
                graph_alarm::not_built(&o.ctx, bg, Some(&why));
            }
            return Ok(());
        }
    };
    if let Some(j) = st.full.as_mut() {
        j.since.extend(out.evaluated.iter().cloned());
    }
    let edges = out.graph.len();
    let snap = Snapshot {
        graph: Arc::new(out.graph),
        basis: snap_basis,
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
        changed: out.changed,
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
            build_ms: None,
            reconcile_ms: None,
            evaluated: out.evaluated.len(),
            changed: Some(out.changed),
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
