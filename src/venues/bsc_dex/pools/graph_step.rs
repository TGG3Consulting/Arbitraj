//! Файл: venues/bsc_dex/pools/graph_step.rs
//! Что делает: решение 308 (задача t137) — одно пробуждение фонового владельца графа (graph_owner.rs run):
//! - режим dex.graph_background выключен — снимок снят, слежение выключено, идущая сборка отменена;
//! - не текущее поколение (сторож уже запустил другого владельца) — ничего не делает (повторное ревью п. 2);
//! - настройки графа/глубины или USDT не заданы (функция не работает) — снимка нет с причиной; временный сбой (чтение
//!   decimals USDT у узла) — прежний basis карты, снимок и карта остаются (graph_basis.rs; ревью №2 F);
//! - сменился USDT — прежняя карта выбрасывается (другие концы путей; читатели такой снимок не берут), догоняющая
//!   сборка (повторное ревью п. 5: сторож не принимает её за зависание);
//! - нет карты, сменились настройки графа или пришёл запрос — запуск полной сборки (graph_full.rs build) в потоке
//!   блокирующих задач, параллельно кругам; запрос во время идущей сборки — после неё (ревью №2 E);
//! - срок самопроверки dex.graph_verify_s назначается заново только при смене самой настройки (ревью №2 G);
//! - признак «догоняющая сборка» (карты нет, сборка идёт, снимок есть — например после перезапуска сторожем):
//!   читатели берут действующий снимок, отставшим он не считается (ревью №2 A, B; graph_read.rs);
//! - неудачная полная сборка повторяется не раньше dex.graph_retry_pause_s (ревью №1 O; срок — в Bg, переживает
//!   перезапуск владельца — повторное ревью п. 3); отложенный запрос (again) паузу не обходит (повторное ревью а);
//! - накопительный круг и публикация — graph_publish.rs.
//!
//! Кто вызывает: graph_owner.rs (run).
//! От чего зависит: graph_owner.rs (Owner, St, FullJob), graph_publish.rs, graph_full.rs, graph_snap.rs, graph_alarm.rs,
//! graph_read.rs, graph_basis.rs, ctx.rs.
//! Настройки: dex.graph_background, dex.graph_verify_s, dex.graph_retry_pause_s, dex.route_max_pools_per_edge [bsc_dex]; через книгу глубины —
//! dex.depth_band_pct, dex.min_pool_liquidity_usd, dex.depth_hysteresis_pct; venue.quote_asset (USDT).

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::super::ctx::Ctx;
use super::graph_basis::{NoBasis, basis_or};
use super::graph_owner::{FullJob, Owner, St};
use super::graph_snap::{Basis, Bg};
use super::{graph_alarm, graph_publish, graph_read};

/// Одно пробуждение (см. шапку). Ошибки (текст): круг упал (паника) — владелец выходит, сторож перезапустит.
pub(super) async fn step(o: &Owner, bg: &Arc<Bg>, st: &mut St) -> Result<(), String> {
    let ctx = &o.ctx;
    if !bg.is_current(o.generation) {
        return Ok(());
    }
    if !graph_read::background(ctx) {
        if bg.changes.tracking() || bg.load().is_some() || st.work.is_some() || st.full.is_some() {
            bg.changes.track(false);
            bg.clear(None);
            st.work = None;
            if let Some(j) = st.full.take() {
                j.stop();
            }
            st.next_verify = None;
            st.again = None;
            bg.set_catching_for(o.generation, false);
        }
        let _ = bg.mode_switched_on(false);
        // Четвёртое ревью п. 1: режим выключен — ступени Б нет.
        bg.end_stage_b();
        let _ = bg.take_full();
        return Ok(());
    }
    // Третье ревью п. 6: режим только что включён — переводы в USD могли посчитаться по прежней dex.v2_fee_pct (её смену
    // при выключенном режиме книга не видела): пересчитать при следующем запросе.
    // Признак — в Bg (четвёртое ревью а): ровно на переход «выкл → вкл», а не на каждый экземпляр владельца. На старте
    // бота при включённом режиме сброс тоже один раз — книга тогда пуста, сброс ничего не стоит и не меняет итогов.
    if bg.mode_switched_on(true) {
        o.reg.depth().invalidate();
    }
    bg.changes.track(true);
    let prev = st.work.as_ref().map(|w| w.basis.clone());
    let res = basis_or(o, prev.as_ref()).await;
    // Повторное ревью п. 2: пока ждали узел, сторож мог сменить поколение — побочных действий не делаем.
    if !bg.is_current(o.generation) {
        return Ok(());
    }
    let (basis, per_edge) = match res {
        Ok((b, n, transient)) => {
            graph_alarm::not_built(ctx, bg, transient.as_deref(), true);
            (b, n)
        }
        Err(NoBasis::Config(why)) => {
            graph_alarm::not_built(ctx, bg, Some(&why), false);
            // Функция не работает (настройка не задана) — снимка нет, слежение не нужно (сторож не видит «отставания»).
            bg.changes.track(false);
            bg.clear(Some(why));
            st.work = None;
            st.again = None;
            if let Some(j) = st.full.take() {
                j.stop();
            }
            bg.set_catching_for(o.generation, false);
            return Ok(());
        }
        // Временный сбой, прежнего подходящего basis нет (карты нет) — повтор на следующем пробуждении. Отсчёт «снимок на
        // прежних настройках» здесь не обновляется (пятое ревью в): basis неизвестен, сравнивать не с чем; карты нет —
        // значит, идёт или будет догоняющая сборка (её отсчёт ведёт set_catching_for), а снимок, если есть, остаётся с
        // прежним признаком до первого шага с известным basis.
        Err(NoBasis::Transient(why)) => {
            graph_alarm::not_built(ctx, bg, Some(&why), true);
            return Ok(());
        }
    };
    // Повторное ревью п. 5: другой USDT — прежняя карта бесполезна (концы путей другие), строится новая.
    if st
        .work
        .as_ref()
        .is_some_and(|w| w.basis.usdt != basis.usdt || w.basis.usdt_dec != basis.usdt_dec)
    {
        st.work = None;
    }
    // Четвёртое ревью п. 1: ступень «снимок на прежних настройках» определяет владелец по basis действующего снимка.
    bg.set_stale_for(
        o.generation,
        bg.load()
            .is_some_and(|s| s.basis.settings != basis.settings),
    );
    schedule_verify(ctx, st);
    let req = bg.take_full();
    let need = st.work.as_ref().is_none_or(|w| w.basis != basis);
    let running = st.full.as_ref().is_some_and(|j| j.basis == basis);
    // Ревью №1 (O): неудачная полная сборка повторяется не раньше dex.graph_retry_pause_s (запрос человека — сразу;
    // отложенный запрос again — нет, повторное ревью а).
    let paused = req.is_none() && bg.retry_at().is_some();
    let again = if !running && !paused {
        st.again.take()
    } else {
        None
    };
    let want = req.or(again);
    if (need || want.is_some()) && !running && !paused {
        bg.set_retry_at(None);
        let reason = want.unwrap_or_else(|| match st.work {
            None => "снимка нет — первая сборка".to_string(),
            Some(_) => "сменились настройки графа".to_string(),
        });
        start_full(o, st, basis.clone(), reason);
    } else if let Some(r) = want {
        // Ревью №2 (E): сборка уже идёт или действует пауза повтора — запрос выполняется после, не теряется.
        st.again = Some(r);
    }
    // Карты нет, сборка идёт (первая сборка, после перезапуска, смена USDT) — догоняющая сборка: снимок (если есть)
    // отдаётся, сторож её зависанием не считает (предел — dex.graph_stage_b_max_ms).
    bg.set_catching_for(o.generation, st.work.is_none() && st.full.is_some());
    graph_publish::round(o, bg, st, basis, per_edge).await
}

/// Срок самопроверки: назначается, когда есть карта и задана dex.graph_verify_s; смена значения — заново. Ошибок нет.
fn schedule_verify(ctx: &Ctx, st: &mut St) {
    let want = ctx.int("dex.graph_verify_s").ok();
    if want != st.verify_s {
        st.verify_s = want;
        st.next_verify = None;
    }
    if st.next_verify.is_none()
        && st.work.is_some()
        && let Some(s) = want
    {
        st.next_verify = Some(tokio::time::Instant::now() + Duration::from_secs(s));
    }
}

/// Запуск полной сборки в потоке блокирующих задач (прежняя идущая — отменяется). Карты ещё нет — набор изменений
/// сбрасывается (сборка прочитает реестр позже; изменения после — в наборе, их доведёт finish). Ошибок нет.
fn start_full(o: &Owner, st: &mut St, basis: Basis, reason: String) {
    let bg = o.reg.graph_cache().bg().clone();
    if let Some(old) = st.full.take() {
        old.stop();
    }
    if !bg.is_current(o.generation) {
        return;
    }
    if st.work.is_none() {
        let _ = bg.changes.drain();
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let (ctx, reg, tokens, b) = (
        o.ctx.clone(),
        o.reg.clone(),
        o.tokens.clone(),
        basis.clone(),
    );
    let (flag, generation) = (cancel.clone(), o.generation);
    let handle = tokio::task::spawn_blocking(move || {
        let alive = || !flag.load(Ordering::Acquire) && bg.is_current(generation);
        super::graph_full::build(&ctx, &reg, &tokens, b, &alive)
    });
    st.full = Some(FullJob {
        handle,
        since: HashSet::new(),
        reason,
        started: Instant::now(),
        basis,
        cancel,
    });
}
