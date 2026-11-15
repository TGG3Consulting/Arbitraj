//! Файл: venues/bsc_dex/pools/graph_step.rs
//! Что делает: решение 308 (задача t137) — одно пробуждение фонового владельца графа (graph_owner.rs run):
//! - режим dex.graph_background выключен — снимок снят, слежение выключено, идущая сборка отменена;
//! - настройки графа/глубины или USDT не заданы (функция не работает) — снимка нет с причиной; временный сбой (чтение
//!   decimals USDT у узла) — прежний снимок и карта остаются, повтор на следующем пробуждении (ревью №2 F);
//! - нет карты, сменились настройки графа или пришёл запрос — запуск полной сборки (graph_full.rs build) в потоке
//!   блокирующих задач, параллельно кругам; запрос во время идущей сборки — после неё (ревью №2 E);
//! - срок самопроверки dex.graph_verify_s назначается заново только при смене самой настройки (ревью №2 G);
//! - признак «догоняющая сборка» (карты нет, сборка идёт, снимок есть — например после перезапуска сторожем):
//!   читатели берут действующий снимок, отставшим он не считается (ревью №2 A, B; graph_read.rs);
//! - неудачная полная сборка повторяется не раньше dex.graph_retry_pause_s (ревью №1 O);
//! - накопительный круг и публикация — graph_publish.rs.
//!
//! Кто вызывает: graph_owner.rs (run), graph_finish.rs (basis_now).
//! От чего зависит: graph_owner.rs (Owner, St, FullJob), graph_publish.rs, graph_full.rs, graph_snap.rs, graph_alarm.rs,
//! graph_read.rs, graph_cache.rs (graph_settings), depth_book.rs, tokens.rs, ctx.rs.
//! Настройки: dex.graph_background, dex.graph_verify_s, dex.graph_retry_pause_s, dex.route_max_pools_per_edge [bsc_dex]; через книгу глубины —
//! dex.depth_band_pct, dex.min_pool_liquidity_usd, dex.depth_hysteresis_pct; venue.quote_asset (USDT).

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::depth_book::{self, Book};
use super::graph_cache::graph_settings;
use super::graph_owner::{FullJob, Owner, St};
use super::graph_snap::{Basis, Bg};
use super::{graph_alarm, graph_publish, graph_read};

/// Почему basis не получен.
pub(super) enum NoBasis {
    /// Не задана / неверна настройка, USDT не известен — функция не работает: снимка нет.
    Config(String),
    /// Временный сбой (decimals USDT у узла) — прежний снимок остаётся, повтор.
    Transient(String),
}

/// Одно пробуждение (см. шапку). Ошибки (текст): круг упал (паника) — владелец выходит, сторож перезапустит.
pub(super) async fn step(o: &Owner, bg: &Arc<Bg>, st: &mut St) -> Result<(), String> {
    let ctx = &o.ctx;
    // Срок повтора прошёл — снят (иначе таймер владельца срабатывал бы вхолостую).
    if st
        .retry_at
        .is_some_and(|t| tokio::time::Instant::now() >= t)
    {
        st.retry_at = None;
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
            bg.set_catching_up(false);
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
        Err(NoBasis::Config(why)) => {
            graph_alarm::not_built(ctx, bg, Some(&why));
            // Функция не работает (настройка не задана) — снимка нет, слежение не нужно (сторож не видит «отставания»).
            bg.changes.track(false);
            bg.clear(Some(why));
            st.work = None;
            if let Some(j) = st.full.take() {
                j.stop();
            }
            bg.set_catching_up(false);
            return Ok(());
        }
        // Ревью №2 (F): временный сбой — снимок и карта остаются; круги идут на basis карты; повтор — на следующем
        // пробуждении. Карты нет — ждём.
        Err(NoBasis::Transient(why)) => {
            graph_alarm::not_built(ctx, bg, Some(&why));
            let (Some(w), Ok(per_edge)) = (st.work.as_ref(), checks(ctx, &o.tokens)) else {
                return Ok(());
            };
            let b = w.basis.clone();
            return graph_publish::round(o, bg, st, b, per_edge).await;
        }
    };
    schedule_verify(ctx, st);
    let req = bg.take_full();
    let need = st.work.as_ref().is_none_or(|w| w.basis != basis);
    let running = st.full.as_ref().is_some_and(|j| j.basis == basis);
    // Ревью №1 (O): неудачная полная сборка повторяется не раньше dex.graph_retry_pause_s (запрос человека — сразу).
    let paused = req.is_none() && st.retry_at.is_some();
    if (need || req.is_some()) && !running && !paused {
        st.retry_at = None;
        let reason = req.unwrap_or_else(|| match st.work {
            None => "снимка нет — первая сборка".to_string(),
            Some(_) => "сменились настройки графа".to_string(),
        });
        start_full(o, st, basis.clone(), reason);
    } else if let Some(r) = req {
        // Ревью №2 (E): сборка уже идёт (начата раньше запроса) — запрос выполняется после неё, не теряется.
        st.again = Some(r);
    }
    // Карты нет, сборка идёт (первая сборка, после перезапуска) — догоняющая сборка: снимок (если есть) отдаётся, сторож
    // её зависанием не считает.
    bg.set_catching_up(st.work.is_none() && st.full.is_some());
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

/// Настройки снимка сейчас. Возвращает: basis и dex.route_max_pools_per_edge. Ошибки: Config — не задана настройка
/// графа/глубины или USDT не известен; Transient — decimals USDT не прочитаны (узел).
pub(super) async fn basis_now(o: &Owner) -> Result<(Basis, usize), NoBasis> {
    let per_edge = checks(&o.ctx, &o.tokens).map_err(NoBasis::Config)?;
    let usdt = o
        .tokens
        .usdt(&o.ctx)
        .map_err(|e| NoBasis::Config(e.message))?;
    let usdt_dec = o
        .tokens
        .decimals(&o.ctx, usdt)
        .await
        .map_err(|e| NoBasis::Transient(e.message))?;
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

/// Запуск полной сборки в потоке блокирующих задач (прежняя идущая — отменяется). Карты ещё нет — набор изменений
/// сбрасывается (сборка прочитает реестр позже; изменения после — в наборе, их доведёт finish). Ошибок нет.
fn start_full(o: &Owner, st: &mut St, basis: Basis, reason: String) {
    let bg = o.reg.graph_cache().bg().clone();
    if let Some(old) = st.full.take() {
        old.stop();
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
