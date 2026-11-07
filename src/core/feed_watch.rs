//! Файл: core/feed_watch.rs
//! Что делает: решение 202 (Тигран, 06.11.2026: «переподписка не биржи, а именно пачки») — сторож пар биржи: раз в
//! venue.pair_resub_interval_ms берёт у адаптера свежесть соединений (контракт feed_health), решает по чистой логике
//! (feed_watch_logic.rs) и выполняет: переподписать одну пару (resubscribe_pair), переподключить одно соединение
//! (reconnect_conn — только если по его парам нет идущей сделки, core/feed_hold.rs). Журнал — на каждое действие
//! (попытка — первая в круге), Telegram — «переподключить нельзя: идёт сделка» и «соединение не оживает, нужен человек».
//! Не задана любая настройка сторожа — сторож площадки выключен (одна запись в журнал), ждёт изменения настроек.
//! Адаптер без поддержки или кадры стакана биржи не по расписанию (тишина — норма) — сторож не запускается.
//! Кто вызывает: core/engine.rs (задача на каждую биржу).
//! От чего зависит: feed_watch_logic.rs, feed_hold.rs, engine.rs, gateway (feed_health), settings.
//! Настройки: venue.book_stale_ms, venue.pair_dead_mult, feed.stale_after_ms, venue.feed_warmup_ms,
//! venue.pair_resub_attempts, venue.pair_resub_interval_ms, venue.pair_resub_pause_ms, venue.conn_dead_pct,
//! venue.conn_reconnect_min_ms, venue.conn_reconnect_max_per_hour (по площадке).

use std::collections::HashMap;
use std::time::Duration;

use crate::core::types::VenueId;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;
use super::feed_watch_logic::{self as logic, Action, Cfg, State};

/// Настройки сторожа площадки. Ошибка — текст: какая не задана.
fn cfg(core: &Core, venue: &VenueId) -> Result<Cfg, String> {
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    let s = &core.inner.deps.settings;
    let i = |n: &str| s.integer(n, at).map_err(|e| e.message);
    Ok(Cfg {
        book_stale_ms: i("venue.book_stale_ms")? as i64,
        dead_mult: i("venue.pair_dead_mult")? as i64,
        live_ms: i("feed.stale_after_ms")? as i64,
        warmup_ms: i("venue.feed_warmup_ms")? as i64,
        attempts: i("venue.pair_resub_attempts")? as u32,
        interval_ms: i("venue.pair_resub_interval_ms")? as i64,
        pause_ms: i("venue.pair_resub_pause_ms")? as i64,
        dead_pct: i("venue.conn_dead_pct")? as i64,
        reconnect_min_ms: i("venue.conn_reconnect_min_ms")? as i64,
        reconnect_max_per_hour: i("venue.conn_reconnect_max_per_hour")? as usize,
    })
}

/// Журнал сторожа. Ошибок нет.
async fn log(core: &Core, venue: &VenueId, level: Level, msg: String) {
    core.inner
        .deps
        .log(level, "core.feed_watch", Some(&venue.0), msg, None)
        .await;
}

/// Задача сторожа одной биржи. Работает, пока жива система. Ошибок не возвращает.
pub async fn run(core: Core, venue: VenueId) {
    let mut st = State::default();
    let mut told: Option<String> = None;
    let mut watch = core.inner.deps.watch(&["venue.", "feed."]);
    loop {
        let c = match cfg(&core, &venue) {
            Ok(c) => c,
            Err(why) => {
                if told.as_deref() != Some(why.as_str()) {
                    log(
                        &core,
                        &venue,
                        Level::Info,
                        format!("Сторож пар выключен: {why}"),
                    )
                    .await;
                    told = Some(why);
                }
                if watch.next().await.is_none() {
                    return;
                }
                continue;
            }
        };
        told = None;
        let Ok(v) = core.inner.deps.gw.get(&venue).await else {
            tokio::time::sleep(Duration::from_millis(c.interval_ms.max(1) as u64)).await;
            continue;
        };
        let Some(h) = v.feed_health() else { return };
        if !h.scheduled {
            return;
        }
        let deals: HashMap<String, Vec<String>> = super::feed_hold::holders(&core, &venue)
            .into_iter()
            .fold(HashMap::new(), |mut m, (id, a)| {
                // Решение 211 (ревью В4): здоровье потока площадки — по её тикеру монеты, не по показному имени.
                let t = super::venue_ticker::ticker_on(&core, &a, &venue);
                m.entry(t).or_default().push(id);
                m
            });
        let now = chrono::Utc::now().timestamp_millis();
        for a in logic::decide(&h, now, &c, &deals, &mut st) {
            act(&core, &venue, v.as_ref(), a, &c).await;
        }
        tokio::time::sleep(Duration::from_millis(c.interval_ms.max(1) as u64)).await;
    }
}

/// Выполняет действие: команда адаптеру, журнал, Telegram. Ошибок нет: сбой команды — в журнал.
async fn act(
    core: &Core,
    venue: &VenueId,
    v: &dyn crate::gateway::contract::Venue,
    a: Action,
    c: &Cfg,
) {
    let mult = c.book_stale_ms * c.dead_mult;
    match a {
        Action::Resub {
            conn,
            ticker,
            attempt,
            round,
        } => {
            let r = v.resubscribe_pair(&conn, &ticker).await;
            if attempt == 1 || r.is_err() {
                let tail = r
                    .err()
                    .map(|e| format!(" — не отправлено: {}", e.message))
                    .unwrap_or_default();
                let m = format!(
                    "{conn}: пара {ticker} молчит дольше {mult} мс при живом соединении — переподписка (круг {round}, \
                     попыток в круге {}){tail}",
                    c.attempts
                );
                log(core, venue, Level::Warn, m).await;
            }
        }
        Action::Recovered { conn, ticker } => {
            log(
                core,
                venue,
                Level::Info,
                format!("{conn}: пара {ticker} ожила после переподписки"),
            )
            .await;
        }
        Action::Reconnect { conn, dead, total } => {
            let r = v.reconnect_conn(&conn).await;
            let tail = r
                .err()
                .map(|e| format!(" — не выполнено: {}", e.message))
                .unwrap_or_default();
            let m = format!(
                "{conn}: мёртвых пар {dead} из {total} (больше {}%), идущих сделок по ним нет — переподключение только этого \
                 соединения{tail}",
                c.dead_pct
            );
            log(core, venue, Level::Warn, m).await;
        }
        Action::Blocked {
            conn,
            dead,
            total,
            deals,
        } => {
            let m = format!(
                "{conn}: мёртвых пар {dead} из {total} (больше {}%), но по парам соединения идут сделки {} — соединение не \
                 переподключаю, переподписка по парам продолжается",
                c.dead_pct,
                deals.join(", ")
            );
            log(core, venue, Level::Warn, m.clone()).await;
            core.inner
                .deps
                .notify("venue.feed_conn_blocked", Some(&venue.0), &m)
                .await;
        }
        Action::GiveUp {
            conn,
            dead,
            total,
            count,
        } => {
            let m = format!(
                "Соединение {conn} биржи {} не оживает: мёртвых пар {dead} из {total}, переподключений за час {count} \
                 (venue.conn_reconnect_max_per_hour) — больше не переподключаю, нужен человек",
                venue.0
            );
            log(core, venue, Level::Error, m.clone()).await;
            core.inner
                .deps
                .notify("venue.feed_conn_giveup", Some(&venue.0), &m)
                .await;
        }
    }
}
