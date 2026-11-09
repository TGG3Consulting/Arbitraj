//! Файл: core/window_worker.rs
//! Что делает: задача окон (решение 125) — всё медленное вне тика и очереди кандидатов: открытие окна (файл zstd,
//! приёмник, запись на биржах через шлюз, сводка в SQLite, журнал, Telegram window.opened), вход нового направления
//! (запись на новой бирже), когда закрывать (закрытие и сделки окна — window_close.rs) и уборка — при запуске и раз
//! в window.cleanup_s.
//! Закрытие: ни одно направление не проходит дольше window.min_hold_ms и по монете нет открытой сделки (window.rs);
//! будят задачу сообщения, толчок window_kick (направление перестало проходить, сделка сохранена) и срок.
//! Окна выключены (window.enabled = нет, не задана настройка) — открытые закрываются с этой причиной.
//! Решение 241: точка цены пула DEX (WinMsg::Dex, window_dex.rs) — строка `{"type":"dex","point":…}` в файл открытого
//! окна; окно уже закрыто — точка отбрасывается.
//! Кто вызывает: core/engine.rs (задача run).
//! От чего зависит: window.rs, window_run.rs (WinMsg), window_cfg.rs, window_tap.rs, window_close.rs (сделки, закрытие,
//! сводка).
//! Настройки: window.* (window_cfg.rs).

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use serde_json::json;
use tokio::sync::mpsc::Receiver;
use tokio::time::{Duration, Instant, sleep_until};

use super::coin_key::CoinKey;
use super::engine::{Core, lk};
use super::window::WindowInfo;
use super::window_cfg::{Cfg, cleanup, load, warn};
use super::window_close::{Run, close, deals, log, save};
use super::window_info::{mark, pairs};
use super::window_run::WinMsg;
use super::window_tap;

/// Задача окон. Работает, пока жив канал.
pub async fn run(core: Core, mut rx: Receiver<WinMsg>) {
    load(&core).await;
    let mut st: HashMap<String, Run> = HashMap::new();
    let mut next_clean = Instant::now();
    loop {
        let cfg = Cfg::read(&core.inner.deps.settings).ok();
        let close_at = cfg
            .as_ref()
            .and_then(|c| lk(&core.inner.windows).deadline(c.min_hold_ms))
            .map(|t| {
                let ms = (t - Utc::now()).num_milliseconds().max(0) as u64;
                Instant::now() + Duration::from_millis(ms)
            });
        let wake = [cfg.as_ref().map(|_| next_clean), close_at]
            .into_iter()
            .flatten()
            .min();
        tokio::select! {
            m = rx.recv() => match m {
                Some(m) => handle(&core, &mut st, m).await,
                None => return,
            },
            _ = core.inner.window_kick.notified() => {}
            _ = async { match wake { Some(t) => sleep_until(t).await, None => std::future::pending().await } } => {}
        }
        let cfg = Cfg::read(&core.inner.deps.settings);
        deals(&core, &mut st);
        let shut: Vec<(CoinKey, String)> = match &cfg {
            Ok(c) => {
                let busy: HashSet<CoinKey> = st
                    .values()
                    .filter(|r| r.deals.iter().any(|d| d.open))
                    .map(|r| r.key.clone())
                    .collect();
                let ks = lk(&core.inner.windows)
                    .closable(Utc::now(), c.min_hold_ms, |k| busy.contains(k));
                ks.into_iter()
                    .map(|k| (k, "направление больше не проходит".into()))
                    .collect()
            }
            Err(why) => lk(&core.inner.windows)
                .list()
                .iter()
                .map(|w| (CoinKey::of(&w.asset), format!("окна выключены: {why}")))
                .collect(),
        };
        for (k, why) in shut {
            close(&core, &mut st, &k, &why).await;
        }
        if let Ok(c) = &cfg
            && Instant::now() >= next_clean
        {
            let open: HashSet<String> = st.keys().cloned().collect();
            cleanup(&core, c, &open).await;
            next_clean = Instant::now() + Duration::from_secs(c.cleanup_s.max(1));
        }
    }
}

/// Одно сообщение. Ошибок нет — сбои в журнал.
async fn handle(core: &Core, st: &mut HashMap<String, Run>, m: WinMsg) {
    match m {
        WinMsg::Off(Some(w)) => warn(core, format!("окна не пишутся: {w}")).await,
        WinMsg::Off(None) => log(core, "окна снова пишутся".into(), None).await,
        WinMsg::Open { info } => {
            let Ok(cfg) = Cfg::read(&core.inner.deps.settings) else {
                return;
            };
            let files = &core.inner.window_files;
            files.open(&info.id, cfg.zstd_level);
            files.line(
                &info.id,
                json!({ "type": "open", "info": &*info }).to_string(),
            );
            let rec = window_tap::start(core, &info.id, cfg.queue_max);
            let mut r = Run {
                key: CoinKey::of(&info.asset),
                rec,
                taps: HashMap::new(),
                deals: vec![],
            };
            taps(core, &mut r, &info).await;
            let id = info.id.clone();
            st.insert(id.clone(), r);
            let shown = save(core, st, &id).await.unwrap_or(*info);
            let text = format!("{}Окно открыто: {}", mark(&shown), pairs(&shown));
            log(core, text.clone(), Some(&shown)).await;
            core.inner.deps.notify("window.opened", None, &text).await;
        }
        WinMsg::Dex { id, point } => {
            if st.contains_key(&id) {
                let line = json!({ "type": "dex", "point": &*point });
                core.inner.window_files.line(&id, line.to_string());
            }
        }
        WinMsg::Join { info } => {
            let Some(r) = st.get_mut(&info.id) else {
                return;
            };
            core.inner.window_files.line(
                &info.id,
                json!({ "type": "join", "at": Utc::now(), "directions": info.directions })
                    .to_string(),
            );
            taps(core, r, &info).await;
            save(core, st, &info.id).await;
        }
    }
}

/// Включает запись на биржах окна, где ещё не включена; итог — строкой файла. Ошибок нет.
async fn taps(core: &Core, r: &mut Run, info: &WindowInfo) {
    for w in &info.venues {
        if r.taps.contains_key(&w.venue.0) {
            continue;
        }
        let res = window_tap::tap(core, &w.venue, &w.asset, r.rec.sink.clone()).await;
        let line = json!({ "type": "tap", "at": Utc::now(), "venue": w.venue.0, "result": &res });
        core.inner.window_files.line(&info.id, line.to_string());
        if let Err(e) = &res {
            warn(
                core,
                format!("окно {}: запись на {} не включена: {e}", info.id, w.venue.0),
            )
            .await;
        }
        r.taps.insert(w.venue.0.clone(), res);
    }
}
