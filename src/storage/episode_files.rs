//! Файл: storage/episode_files.rs
//! Что делает: файлы эпизодов арбитражной ситуации (решение 74) — один файл на эпизод, сжатие zstd, как
//! storage/raw_log.rs: <data-dir>/episodes/<id>.jsonl.zst, одна строка — одна запись JSON (заголовок, пересчёты,
//! закрытие). Запись — в отдельном потоке, ядро не ждёт диск. После каждой записи сжатые данные сбрасываются на диск
//! (открытый эпизод читается до последней записи). Сколько места занято — счётчик (для лимита episode.max_mb), с концом
//! кадра при закрытии. remove и orphans — блокирующие: ядро зовёт их в spawn_blocking.
//! Кто вызывает: core/episode_run.rs (open, line, close, remove, used), core/engine.rs (start), api/episodes.rs (read,
//! path — через ядро).
//! От чего зависит: zstd.
//! Настройки: нет (уровень сжатия episode.zstd_level передаёт ядро при открытии).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;

/// Команда потоку записи.
enum Cmd {
    Open { id: String, level: i32 },
    Line { id: String, text: String },
    Close { id: String },
}

/// Файлы эпизодов. Дешёво клонируется.
#[derive(Clone)]
pub struct EpisodeFiles {
    dir: PathBuf,
    tx: mpsc::UnboundedSender<Cmd>,
    /// Занято всеми файлами папки, байт (по размерам файлов после записи).
    used: Arc<AtomicU64>,
}

impl EpisodeFiles {
    /// Что делает: запускает поток записи. Принимает: папку эпизодов. Ошибок нет: сбой — в лог процесса.
    pub fn start(dir: PathBuf) -> Self {
        let used = Arc::new(AtomicU64::new(scan(&dir)));
        let (tx, rx) = mpsc::unbounded_channel();
        let (d, u) = (dir.clone(), used.clone());
        std::thread::Builder::new()
            .name("episodes".into())
            .spawn(move || writer(rx, d, u))
            .map_err(|e| tracing::error!(%e, "не удалось запустить запись эпизодов"))
            .ok();
        Self { dir, tx, used }
    }

    /// Открыть файл эпизода. Принимает: id, уровень zstd. Ошибок нет (поток записи пишет их в лог).
    pub fn open(&self, id: &str, level: i32) {
        let _ = self.tx.send(Cmd::Open {
            id: id.into(),
            level,
        });
    }

    /// Дописать строку JSON. Принимает: id, текст строки. Ошибок нет.
    pub fn line(&self, id: &str, text: String) {
        let _ = self.tx.send(Cmd::Line {
            id: id.into(),
            text,
        });
    }

    /// Закрыть файл (конец кадра zstd). Принимает: id. Ошибок нет.
    pub fn close(&self, id: &str) {
        let _ = self.tx.send(Cmd::Close { id: id.into() });
    }

    /// Путь файла эпизода. Ошибок нет.
    pub fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.jsonl.zst"))
    }

    /// Занято места, байт. Ошибок нет.
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    /// id файлов папки, которых нет в known (сироты — строки в базе нет). Блокирующее: вызывать в spawn_blocking.
    /// Принимает: известные id. Возвращает: id сирот. Ошибок нет (нет папки — пусто).
    pub fn orphans(&self, known: &std::collections::HashSet<String>) -> Vec<String> {
        let Ok(it) = std::fs::read_dir(&self.dir) else {
            return vec![];
        };
        it.flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_suffix(".jsonl.zst")
                    .map(String::from)
            })
            .filter(|id| !known.contains(id))
            .collect()
    }

    /// Удалить файл закрытого эпизода. Принимает: id. Ошибки: нет доступа (файла нет — не ошибка).
    pub fn remove(&self, id: &str) -> std::io::Result<()> {
        let p = self.path(id);
        let n = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(&p) {
            Ok(()) => {
                self.used.fetch_sub(n.min(self.used()), Ordering::Relaxed);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Что делает: читает записи файла (у открытого эпизода — до последней сброшенной). Принимает: путь, сколько строк
/// не больше. Возвращает: (строки, обрезано ли по пределу). Ошибки: нет файла, сжатие повреждено в начале.
pub fn read(path: &Path, max: usize) -> std::io::Result<(Vec<String>, bool)> {
    let dec = zstd::Decoder::new(File::open(path)?)?;
    let mut out = Vec::new();
    for line in BufReader::new(dec).lines() {
        // Незакрытый кадр (эпизод ещё пишется) обрывается ошибкой в конце — берём прочитанное.
        let Ok(l) = line else { break };
        if out.len() >= max {
            return Ok((out, true));
        }
        out.push(l);
    }
    Ok((out, false))
}

/// Сумма размеров файлов папки, байт. Ошибок нет (нет папки — 0).
fn scan(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Поток записи: открытые файлы по id.
fn writer(mut rx: mpsc::UnboundedReceiver<Cmd>, dir: PathBuf, used: Arc<AtomicU64>) {
    let mut files: HashMap<String, (zstd::Encoder<'static, File>, u64)> = HashMap::new();
    while let Some(cmd) = rx.blocking_recv() {
        match cmd {
            Cmd::Open { id, level } => {
                let r = std::fs::create_dir_all(&dir)
                    .and_then(|_| File::create(dir.join(format!("{id}.jsonl.zst"))))
                    .and_then(|f| zstd::Encoder::new(f, level));
                match r {
                    Ok(enc) => {
                        files.insert(id, (enc, 0));
                    }
                    Err(e) => tracing::error!(%e, %id, "не удалось открыть файл эпизода"),
                }
            }
            Cmd::Line { id, text } => {
                let Some((enc, size)) = files.get_mut(&id) else {
                    continue;
                };
                if let Err(e) = writeln!(enc, "{text}").and_then(|_| enc.flush()) {
                    tracing::error!(%e, %id, "ошибка записи эпизода");
                }
                let now = enc.get_ref().metadata().map(|m| m.len()).unwrap_or(*size);
                used.fetch_add(now.saturating_sub(*size), Ordering::Relaxed);
                *size = now;
            }
            Cmd::Close { id } => {
                let Some((enc, size)) = files.remove(&id) else {
                    continue;
                };
                match enc.finish() {
                    // Конец кадра zstd дописан — его байты тоже в занятом месте.
                    Ok(f) => {
                        let now = f.metadata().map(|m| m.len()).unwrap_or(size);
                        used.fetch_add(now.saturating_sub(size), Ordering::Relaxed);
                    }
                    Err(e) => tracing::error!(%e, %id, "ошибка закрытия файла эпизода"),
                }
            }
        }
    }
    for (_, (enc, _)) in files.drain() {
        let _ = enc.finish();
    }
}
