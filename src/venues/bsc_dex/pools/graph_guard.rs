//! Файл: venues/bsc_dex/pools/graph_guard.rs
//! Что делает: решение 308 (задача t137) — СТОРОЖ фонового владельца графа (И8 проекта
//! docs/notes/graph-snapshot-design-2026-11-16.md): запускает владельца (graph_owner.rs run) с новым поколением и следит:
//! - владелец вышел (паника круга, сбой) — запись Error и событие dex.graph_owner_failed (graph_alarm.rs, не чаще
//!   dex.graph_alert_min_s), счётчик перезапусков, перезапуск;
//! - завис: накопительный круг идёт дольше dex.graph_hang_ms, ИЛИ владелец вне круга, сборки и доводки, а изменения реестра
//!   ждут дольше dex.graph_hang_ms (стоит вне круга) — задача владельца снимается, то же событие и перезапуск.
//!
//! Ревью №2 (B): срок зависания — своя настройка dex.graph_hang_ms (не предел предохранителя); первая и любая полная
//! сборка и доводка зависанием не считаются (их пульс не ставится); брошенная работа останавливается сама на ближайшем
//! пуле (поколение владельца сменилось — graph_snap.rs is_current); после перезапуска, пока новая полная сборка идёт,
//! читатели берут действующий снимок (признак «догоняющая сборка», graph_step.rs) — классики на всю сборку нет.
//! Между перезапусками — пауза dex.graph_retry_pause_s (не задана — перезапуск по следующему изменению реестра или
//! команде), петли «снять → пересобрать → снять» нет. Ревью №1 (Q): падение самой задачи сторожа — keep перезапускает её.
//! Кто вызывает: graph_owner.rs (ensure — keep, один раз на реестр).
//! От чего зависит: graph_owner.rs, graph_snap.rs, graph_alarm.rs, registry.rs, tokens.rs, ctx.rs, settings/live.rs.
//! Настройки: dex.graph_hang_ms, dex.graph_retry_pause_s [bsc_dex] (необязательные; смена — перечитывается сразу).

use std::time::Duration;

use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::graph_alarm;
use super::graph_owner::{self, Owner};
use super::registry::Registry;

/// Что увидел сторож.
enum G {
    Exit(String),
    Again,
    Closed,
    Check,
}

/// Срок зависания dex.graph_hang_ms. None — не задан (ловится только выход владельца). Ошибок нет.
pub fn hang(ctx: &Ctx) -> Option<Duration> {
    ctx.int("dex.graph_hang_ms").ok().map(Duration::from_millis)
}

/// Задача сторожа под присмотром: сторож упал (паника) — запись Error и новый сторож. Работает до остановки системы.
/// Ошибок нет.
pub async fn keep(ctx: Ctx, reg: Registry, tokens: Tokens) {
    loop {
        let h = tokio::spawn(supervise(ctx.clone(), reg.clone(), tokens.clone()));
        match h.await {
            Ok(()) => return,
            Err(e) => {
                let msg = format!("Сторож фоновой задачи графа упал: {e} — запущен заново");
                ctx.log(Level::Error, graph_alarm::KIND, msg, None).await;
            }
        }
    }
}

/// Цикл сторожа (см. шапку). Принимает: окружение, реестр, токены. Работает до остановки системы. Ошибок нет.
pub async fn supervise(ctx: Ctx, reg: Registry, tokens: Tokens) {
    let bg = reg.graph_cache().bg().clone();
    let mut watch = Watch::new(
        ctx.settings.subscribe(),
        vec!["dex.graph_hang_ms".into(), "dex.graph_retry_pause_s".into()],
    );
    loop {
        let o = Owner {
            ctx: ctx.clone(),
            reg: reg.clone(),
            tokens: tokens.clone(),
            generation: bg.next_gen(),
        };
        let mut h = tokio::spawn(graph_owner::run(o));
        let why = loop {
            let limit = hang(&ctx);
            let ev = tokio::select! {
                r = &mut h => G::Exit(match r {
                    Ok(w) => w,
                    Err(e) => format!("фоновая задача графа упала: {e}"),
                }),
                r = watch.next_batch() => if r.is_some() { G::Again } else { G::Closed },
                _ = sleep_opt(limit) => G::Check,
            };
            match ev {
                G::Exit(w) => break w,
                G::Again => {}
                G::Closed => {
                    h.abort();
                    return;
                }
                G::Check => {
                    let Some(l) = limit else { continue };
                    if let Some(age) = bg.round_age()
                        && age > l
                    {
                        h.abort();
                        break format!(
                            "накопительный круг идёт {} мс — дольше dex.graph_hang_ms = {} мс (завис)",
                            age.as_millis(),
                            l.as_millis()
                        );
                    }
                    // Вне круга, сборки и доводки изменения ждут дольше срока — владелец стоит.
                    if bg.round_age().is_none()
                        && !bg.catching_up()
                        && let Some(lag) = bg.changes.lag()
                        && lag > l
                    {
                        h.abort();
                        break format!(
                            "изменения реестра ждут {} мс вне круга — дольше dex.graph_hang_ms = {} мс (владелец стоит)",
                            lag.as_millis(),
                            l.as_millis()
                        );
                    }
                }
            }
        };
        if why == graph_owner::STOP {
            return;
        }
        bg.round_end();
        // Действующий снимок остаётся: до карты новой сборки — ступень Б, а не классика.
        bg.set_catching_up(true);
        bg.stats.restart(&why);
        graph_alarm::owner_failed(&ctx, &bg, &why);
        if let Ok(s) = ctx.int("dex.graph_retry_pause_s") {
            tokio::time::sleep(Duration::from_secs(s)).await;
        } else {
            tokio::select! {
                _ = bg.changes.notified() => {}
                _ = bg.rebuild_notified() => {}
            }
        }
    }
}

/// Сторожевой срок (None — не задан: ждать вечно). Ошибок нет.
async fn sleep_opt(d: Option<Duration>) {
    match d {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}
