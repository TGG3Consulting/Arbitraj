//! Файл: storage/raw_log.rs
//! Что делает: запись сырых сообщений площадок на диск, сжатие zstd — для разбора, подбора порогов
//! и тестов на реальных данных. Запись идёт в отдельном потоке, рыночные задачи не ждут диск.
//! Файлы: <data-dir>/raw/<площадка>/<время начала, unix мс>.jsonl.zst, одна строка — одно сообщение:
//! {"t": время получения (unix мс), "c": соединение, "m": текст сообщения}.
//! Кто вызывает: адаптеры площадок (venues/*) — на каждое полученное сообщение.
//! От чего зависит: settings (raw.enabled, raw.zstd_level, raw.rotate_s), zstd.
//! Настройки: raw.enabled, raw.zstd_level, raw.rotate_s. Не заданы — запись не ведётся (и пишется предупреждение).

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use serde_json::json;
use tokio::sync::mpsc;

use crate::settings::read::At;
use crate::settings::store::Store;

/// Одна запись.
struct Line {
    venue: String,
    conn: String,
    at_ms: i64,
    text: String,
}

/// Отправитель сырых сообщений. Дешёво клонируется.
#[derive(Clone)]
pub struct RawLog {
    tx: mpsc::UnboundedSender<Line>,
    settings: Store,
}

impl RawLog {
    /// Запускает поток записи. Принимает: папку данных и настройки.
    pub fn start(data_dir: PathBuf, settings: Store) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let s = settings.clone();
        std::thread::Builder::new()
            .name("raw-log".into())
            .spawn(move || writer(rx, data_dir.join("raw"), s))
            .map_err(|e| tracing::error!(%e, "не удалось запустить запись сырых сообщений"))
            .ok();
        Self { tx, settings }
    }

    /// Ставит сообщение в очередь на запись, если raw.enabled = да. Ошибок не возвращает.
    pub fn write(&self, venue: &str, conn: &str, text: &str) {
        if !matches!(self.settings.flag("raw.enabled", At::default()), Ok(true)) {
            return;
        }
        let line = Line {
            venue: venue.to_string(),
            conn: conn.to_string(),
            at_ms: chrono::Utc::now().timestamp_millis(),
            text: text.to_string(),
        };
        // Поток записи остановился — сообщение теряется, причина уже в логе процесса.
        let _ = self.tx.send(line);
    }
}

/// Открытый файл одной площадки.
struct Open {
    enc: zstd::Encoder<'static, File>,
    started_ms: i64,
}

/// Поток записи: держит по одному файлу на площадку, меняет файл раз в raw.rotate_s.
fn writer(mut rx: mpsc::UnboundedReceiver<Line>, dir: PathBuf, settings: Store) {
    let mut files: HashMap<String, Open> = HashMap::new();
    while let Some(line) = rx.blocking_recv() {
        let rotate_ms = settings
            .integer("raw.rotate_s", At::default())
            .map(|s| s as i64 * 1000);
        let level = settings.integer("raw.zstd_level", At::default());
        let (Ok(rotate_ms), Ok(level)) = (rotate_ms, level) else {
            tracing::warn!("сырые сообщения не пишутся: не заданы raw.rotate_s или raw.zstd_level");
            continue;
        };
        let expired = files
            .get(&line.venue)
            .is_some_and(|f| line.at_ms - f.started_ms >= rotate_ms);
        if expired && let Some(old) = files.remove(&line.venue) {
            finish(old);
        }
        if !files.contains_key(&line.venue) {
            match open(&dir, &line.venue, line.at_ms, level as i32) {
                Ok(f) => {
                    files.insert(line.venue.clone(), f);
                }
                Err(e) => {
                    tracing::error!(%e, venue = %line.venue, "не удалось открыть файл сырых сообщений");
                    continue;
                }
            }
        }
        if let Some(f) = files.get_mut(&line.venue) {
            let row = json!({ "t": line.at_ms, "c": line.conn, "m": line.text });
            if let Err(e) = writeln!(f.enc, "{row}") {
                tracing::error!(%e, "ошибка записи сырого сообщения");
            }
        }
        // Очередь пуста — сбрасываем сжатые данные на диск, чтобы при сбое терялось минимум.
        if rx.is_empty() {
            for f in files.values_mut() {
                let _ = f.enc.flush();
            }
        }
    }
    for (_, f) in files.drain() {
        finish(f);
    }
}

/// Открывает новый файл площадки. Ошибки: нет доступа к папке.
fn open(dir: &std::path::Path, venue: &str, at_ms: i64, level: i32) -> std::io::Result<Open> {
    let d = dir.join(venue);
    std::fs::create_dir_all(&d)?;
    let file = File::create(d.join(format!("{at_ms}.jsonl.zst")))?;
    Ok(Open {
        enc: zstd::Encoder::new(file, level)?,
        started_ms: at_ms,
    })
}

/// Закрывает файл (дописывает конец кадра zstd).
fn finish(f: Open) {
    if let Err(e) = f.enc.finish() {
        tracing::error!(%e, "ошибка закрытия файла сырых сообщений");
    }
}
