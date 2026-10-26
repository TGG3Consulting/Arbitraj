//! Файл: core/episode_worker.rs
//! Что делает: отдельная задача эпизодов (решение 74) — всё медленное вне тика и очереди кандидатов: файл эпизода
//! (через поток storage/episode_files.rs), сводка в SQLite, журнал, живой поток, Telegram (episode_notify.rs), уборка по
//! сроку, месту и сиротам и закрытие пропавших направлений (sweep) — при запуске и раз в episode.cleanup_s. Сообщения
//! (EpMsg) приходят по каналу в порядке событий; тик и очередь кандидатов только отправляют и не ждут.
//! Места нет (episode.max_mb даже после уборки) — запись открытого эпизода останавливается (причина в сводке), сам
//! эпизод живёт до закрытия.
//! Кто вызывает: core/engine.rs (задача run).
//! От чего зависит: episode_run.rs (EpMsg, sweep), episode_cfg.rs, episode_snap.rs, episode_notify.rs,
//! storage/episodes_db.rs, storage/episode_files.rs.
//! Настройки: episode.* (episode_cfg.rs).

use std::collections::{HashMap, HashSet};

use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Duration, Instant, sleep_until};

use crate::storage::episodes_db;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::episode::EpisodeInfo;
use super::episode_cfg::{Cfg, cleanup, load, warn};
use super::episode_notify::{Open, closed, closed_text, due, mark, opened};
use super::episode_run::{EpMsg, sweep};
use super::view::CoreEvent;

/// Задача эпизодов. Работает, пока жив канал.
pub async fn run(core: Core, mut rx: UnboundedReceiver<EpMsg>) {
    load(&core).await;
    let mut st: HashMap<String, Open> = HashMap::new();
    let mut next_clean = Instant::now();
    // Первая уборка — при запуске (срок, место, сироты); закрытие пропавших направлений — с таймера: при запуске
    // справочник ещё не собран (его пересборка сама вызывает sweep).
    let mut first = true;
    loop {
        let cfg = Cfg::read(&core.inner.deps.settings).ok();
        let wake = [cfg.as_ref().map(|_| next_clean), due(&st, cfg.as_ref())]
            .into_iter()
            .flatten()
            .min();
        tokio::select! {
            m = rx.recv() => match m {
                // Настройки — на момент сообщения (могли смениться, пока задача ждала).
                Some(m) => {
                    let now = Cfg::read(&core.inner.deps.settings).ok();
                    handle(&core, &mut st, now.as_ref(), m).await
                }
                None => return,
            },
            _ = async { match wake { Some(t) => sleep_until(t).await, None => std::future::pending().await } } => {}
        }
        let cfg = Cfg::read(&core.inner.deps.settings).ok();
        for o in st.values_mut() {
            opened(&core, o, cfg.as_ref(), false).await;
        }
        if let Some(c) = &cfg
            && Instant::now() >= next_clean
        {
            if !first {
                sweep(&core);
            }
            first = false;
            let open: HashSet<String> = st.keys().cloned().collect();
            cleanup(&core, c, &open).await;
            next_clean = Instant::now() + Duration::from_secs(c.cleanup_s.max(1));
        }
    }
}

/// Одно сообщение. Ошибок нет — сбои в журнал.
async fn handle(core: &Core, st: &mut HashMap<String, Open>, cfg: Option<&Cfg>, m: EpMsg) {
    let files = &core.inner.episode_files;
    match m {
        EpMsg::Off(why) => match why {
            Some(w) => warn(core, format!("эпизоды не ведутся: {w}")).await,
            None => log(core, "эпизоды снова ведутся".into(), None).await,
        },
        EpMsg::Open { info, calc } => {
            let Some(cfg) = cfg else { return };
            files.open(&info.id, cfg.zstd_level);
            files.line(
                &info.id,
                json!({ "type": "open", "info": info, "calc": calc }).to_string(),
            );
            let mut o = Open::new(info, calc);
            if !room(core, cfg, st).await && o.info.stopped.is_none() {
                stop(core, &mut o).await;
            }
            save(core, &o.info).await;
            log(
                core,
                format!("{}Эпизод открыт: {}", mark(&o.info), o.pair()),
                Some(&o.info),
            )
            .await;
            st.insert(o.info.id.clone(), o);
        }
        EpMsg::Record {
            info,
            calc,
            snap,
            card,
        } => {
            if !st.contains_key(&info.id) {
                return; // эпизод уже закрыт — карточка опоздала
            }
            let fits = match cfg {
                Some(c) => room(core, c, st).await,
                None => false,
            };
            let Some(o) = st.get_mut(&info.id) else {
                return;
            };
            let unbounded = card
                .as_ref()
                .and_then(|c| c.max.as_ref().ok())
                .map(|m| m.opt_profit);
            o.info = lk(&core.inner.episodes)
                .note_unbounded(&info.direction, unbounded)
                .unwrap_or(info);
            if o.info.stopped.is_none() && !fits {
                stop(core, o).await;
            }
            if o.info.stopped.is_none() {
                let line = super::episode_snap::record(&snap, &calc, card.as_deref());
                files.line(&o.info.id, line.to_string());
            }
            o.calc = calc;
            if o.card.is_none() {
                o.card = card;
            }
            save(core, &o.info).await;
        }
        EpMsg::Close { info } => {
            files.line(
                &info.id,
                json!({ "type": "close", "info": info }).to_string(),
            );
            files.close(&info.id);
            save(core, &info).await;
            let o = st.remove(&info.id);
            log(core, closed_text(&info), Some(&info)).await;
            if let Some(mut o) = o {
                o.info = info;
                closed(core, &mut o, cfg).await;
            }
        }
    }
}

/// Есть ли место (уборка, если нет). Принимает: ядро, настройки, открытые. Ошибок нет.
async fn room(core: &Core, cfg: &Cfg, st: &HashMap<String, Open>) -> bool {
    if room_now(core, cfg) {
        return true;
    }
    let open: HashSet<String> = st.keys().cloned().collect();
    cleanup(core, cfg, &open).await;
    room_now(core, cfg)
}

/// Занято не больше episode.max_mb. Ошибок нет.
fn room_now(core: &Core, cfg: &Cfg) -> bool {
    core.inner.episode_files.used() <= cfg.max_bytes
}

/// Запись эпизода остановлена: отметка в памяти ядра и строка журнала. Ошибок нет.
async fn stop(core: &Core, o: &mut Open) {
    let why = format!(
        "лимит места episode.max_mb: занято {} МБ — запись остановлена",
        core.inner.episode_files.used() / (1024 * 1024)
    );
    lk(&core.inner.episodes).stop(&o.info.direction, &why);
    o.info.stopped = Some(why.clone());
    warn(core, format!("{}: {why}", o.info.id)).await;
}

/// Сводку — в SQLite и живой поток. Ошибок нет: сбой базы — в журнал.
async fn save(core: &Core, info: &EpisodeInfo) {
    core.inner.live.send(CoreEvent::Episode(json!(info)));
    let Ok(body) = serde_json::to_string(info) else {
        return;
    };
    let started = info.started_at.to_rfc3339();
    if let Err(e) = episodes_db::save(&core.inner.deps.db, &info.id, started, body).await {
        warn(core, format!("эпизод {}: {e}", info.id)).await;
    }
}

/// Журнал (всегда; Telegram — по правилам episode_notify.rs). Ошибок нет.
async fn log(core: &Core, text: String, info: Option<&EpisodeInfo>) {
    let venue = info.map(|i| i.direction.buy_on.0.clone());
    core.inner
        .deps
        .log(
            Level::Info,
            "core.episode",
            venue.as_deref(),
            text,
            info.map(|i| json!(i)),
        )
        .await;
}
