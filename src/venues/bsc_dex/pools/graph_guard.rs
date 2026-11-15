//! Файл: venues/bsc_dex/pools/graph_guard.rs
//! Что делает: решение 308 (задача t137) — СТОРОЖ фонового владельца графа (И8 проекта
//! docs/notes/graph-snapshot-design-2026-11-16.md): запускает владельца (graph_owner.rs run) с новым поколением и следит:
//! - владелец вышел (паника круга, сбой) — запись Error и событие dex.graph_owner_failed (graph_alarm.rs), счётчик
//!   перезапусков, перезапуск с полной пересборкой;
//! - круг идёт дольше dex.graph_max_lag_ms (завис) — задача владельца снимается, то же событие и перезапуск. Поток,
//!   который висит, остановить нельзя (std); его итог не будет опубликован — публикует только текущее поколение.
//!
//! Проверка зависания — сторожевой срок, равный dex.graph_max_lag_ms (не опрос данных); не задан — ловится только выход.
//! Читатели с первого превышения предела уже получают «маршрута графа нет» (graph_read.rs) — молча со старым снимком они
//! не остаются. Перезапуски вхолостую не крутятся: после сбоя сторож ждёт следующего изменения реестра или команды.
//! Кто вызывает: graph_owner.rs (ensure — один раз на реестр).
//! От чего зависит: graph_owner.rs, graph_snap.rs, graph_alarm.rs, graph_read.rs (max_lag), registry.rs, tokens.rs,
//! ctx.rs, settings/live.rs (Watch).
//! Настройки: dex.graph_max_lag_ms [bsc_dex] (необязательная; смена — перечитывается сразу).

use std::time::Duration;

use crate::settings::live::Watch;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::graph_owner::{self, Owner};
use super::registry::Registry;
use super::{graph_alarm, graph_read};

/// Что увидел сторож.
enum G {
    Exit(String),
    Again,
    Closed,
    Check,
}

/// Цикл сторожа (см. шапку). Принимает: окружение, реестр, токены. Работает до остановки системы. Ошибок нет.
pub async fn supervise(ctx: Ctx, reg: Registry, tokens: Tokens) {
    let bg = reg.graph_cache().bg().clone();
    let mut watch = Watch::new(
        ctx.settings.subscribe(),
        vec!["dex.graph_max_lag_ms".into()],
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
            let limit = graph_read::max_lag(&ctx);
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
                    if let (Some(l), Some(age)) = (limit, bg.round_age())
                        && age > l
                    {
                        h.abort();
                        break format!(
                            "круг фоновой задачи графа идёт {} мс — дольше dex.graph_max_lag_ms = {} мс (завис)",
                            age.as_millis(),
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
        bg.stats.restart(&why);
        graph_alarm::owner_failed(&ctx, &bg, &why);
        tokio::select! {
            _ = bg.changes.notified() => {}
            _ = bg.rebuild_notified() => {}
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
