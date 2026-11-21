//! Файл: core/window_tap.rs
//! Что делает: запись кадров окна (решение 125): приёмник окна (канал ёмкостью window.queue_max на момент открытия) и
//! задача, которая перекладывает каждый кадр в файл окна строкой JSON — площадка, поток (Book / Trades), соединение,
//! время получения в нс и содержимое как есть: текст — полем «t», двоичное — hex полем «h» (как raw.book_frames).
//! Занято больше window.max_mb даже после уборки — запись окна останавливается (причина в сводке, журнал, Telegram
//! window.stopped), окно живёт до закрытия. Включение и выключение записи на бирже — через шлюз (Venue::window_tap).
//! Кто вызывает: core/window_worker.rs.
//! От чего зависит: gateway/tap.rs, window_cfg.rs (cleanup), storage/episode_files.rs (файлы окон), settings.
//! Настройки: window.max_mb (проверяется на каждом кадре — меняется на лету).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::core::types::{Asset, VenueId};
use crate::gateway::tap::{TapData, TapFrame, TapInfo, TapSink, TapStream};
use crate::settings::read::At;

use super::engine::{Core, lk};

/// Счётчики записи окна.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    /// Записано кадров всего.
    pub frames: u64,
    /// Кадров по (площадка, поток).
    pub by: HashMap<(String, String), u64>,
    /// Запись остановлена — почему.
    pub stopped: Option<String>,
}

/// Запись одного окна: приёмник, счётчики, задача записи.
pub struct Rec {
    pub sink: TapSink,
    pub lost: Arc<AtomicU64>,
    pub stats: Arc<Mutex<Stats>>,
    pub task: JoinHandle<()>,
}

/// Что делает: открывает приёмник окна и запускает задачу записи. Принимает: ядро, id окна, ёмкость.
/// Возвращает: Rec. Ошибок нет.
pub fn start(core: &Core, id: &str, cap: usize) -> Rec {
    let (tx, rx) = mpsc::channel(cap.max(1));
    let lost = Arc::new(AtomicU64::new(0));
    let stats = Arc::new(Mutex::new(Stats::default()));
    let task = tokio::spawn(forward(core.clone(), id.to_string(), rx, stats.clone()));
    Rec {
        sink: TapSink::new(tx, lost.clone()),
        lost,
        stats,
        task,
    }
}

/// Задача записи: кадры → строки файла. Заканчивается, когда все приёмники окна закрыты. Ошибок нет.
async fn forward(
    core: Core,
    id: String,
    mut rx: mpsc::Receiver<TapFrame>,
    stats: Arc<Mutex<Stats>>,
) {
    let files = core.inner.window_files.clone();
    while let Some(f) = rx.recv().await {
        if lk(&stats).stopped.is_some() {
            continue;
        }
        if !room(&core).await {
            let why = format!(
                "лимит места window.max_mb: занято {} МБ — запись остановлена",
                files.used() / (1024 * 1024)
            );
            lk(&stats).stopped = Some(why.clone());
            let text = format!("Окно {id}: {why}");
            super::window_cfg::warn(&core, text.clone()).await;
            core.inner
                .deps
                .notify("window.stopped", Some(&f.venue.0), &text)
                .await;
            continue;
        }
        files.line(&id, line(&f).to_string());
        let mut s = lk(&stats);
        s.frames += 1;
        *s.by
            .entry((f.venue.0.clone(), stream(f.stream).into()))
            .or_default() += 1;
    }
}

/// Есть ли место (уборка, если нет). Не задана window.max_mb — места нет. Ошибок нет.
async fn room(core: &Core) -> bool {
    let s = &core.inner.deps.settings;
    let max = |s: &crate::settings::store::Store| {
        s.integer("window.max_mb", At::default())
            .map(|m| m.saturating_mul(1024 * 1024))
    };
    let used = || core.inner.window_files.used();
    match max(s) {
        Ok(m) if used() <= m => return true,
        Err(_) => return false,
        _ => {}
    }
    if let Ok(cfg) = super::window_cfg::Cfg::read(s) {
        let open: HashSet<String> = lk(&core.inner.windows)
            .list()
            .into_iter()
            .map(|w| w.id)
            .collect();
        super::window_cfg::cleanup(core, &cfg, &open).await;
    }
    max(s).is_ok_and(|m| used() <= m)
}

/// Строка кадра в файле окна. Ошибок нет.
pub fn line(f: &TapFrame) -> serde_json::Value {
    let mut v =
        json!({ "type": "f", "v": f.venue.0, "s": stream(f.stream), "c": f.conn, "ns": f.recv_ns });
    match &f.data {
        TapData::Text(t) => v["t"] = json!(t),
        TapData::Binary(b) => v["h"] = json!(hex::encode(b)),
    }
    v
}

/// Обратное line: кадр из строки файла. None — строка не кадр. Ошибок нет.
pub fn parse(v: &serde_json::Value) -> Option<TapFrame> {
    if v["type"] != "f" {
        return None;
    }
    let data = match (v["t"].as_str(), v["h"].as_str()) {
        (Some(t), _) => TapData::Text(t.to_string()),
        (_, Some(h)) => TapData::Binary(hex::decode(h).ok()?),
        _ => return None,
    };
    Some(TapFrame {
        venue: VenueId(v["v"].as_str()?.to_string()),
        stream: match v["s"].as_str()? {
            "Book" => TapStream::Book,
            "Trades" => TapStream::Trades,
            "Diff" => TapStream::Diff,
            "Snapshot" => TapStream::Snapshot,
            _ => return None,
        },
        conn: v["c"].as_str().unwrap_or_default().to_string(),
        recv_ns: v["ns"].as_i64()?,
        data,
    })
}

/// Имя потока в файле. Ошибок нет.
fn stream(s: TapStream) -> &'static str {
    match s {
        TapStream::Book => "Book",
        TapStream::Trades => "Trades",
        TapStream::Diff => "Diff",
        TapStream::Snapshot => "Snapshot",
    }
}

/// Что делает: включает запись монеты на бирже. Принимает: ядро, биржу, монету, приёмник.
/// Возвращает: что включил адаптер или причина. Ошибок нет.
pub async fn tap(core: &Core, v: &VenueId, a: &Asset, sink: TapSink) -> Result<TapInfo, String> {
    let venue = core.inner.deps.gw.get(v).await.map_err(|e| e.message)?;
    // Решение 211: площадке — её тикер монеты.
    let a = &super::venue_ticker::on_venue(core, a, v);
    venue.window_tap(a, sink).await.map_err(|e| e.message)
}

/// Что делает: выключает запись монеты на бирже. Возвращает: выключена ли. Ошибок нет — сбой в журнал.
pub async fn untap(core: &Core, v: &VenueId, a: &Asset) -> bool {
    let r = match core.inner.deps.gw.get(v).await {
        Ok(venue) => {
            venue
                .window_untap(&super::venue_ticker::on_venue(core, a, v))
                .await
        }
        Err(e) => Err(e),
    };
    let Err(e) = r else { return true };
    let text = format!(
        "окно {}: {} — запись не выключена: {}",
        a.ticker, v.0, e.message
    );
    super::window_cfg::warn(core, text).await;
    false
}
