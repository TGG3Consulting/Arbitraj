//! Файл: cli.rs
//! Что делает: разбор аргументов запуска. Здесь только то, что нужно ДО загрузки настроек:
//! где лежат данные, на каком адресе слушать интерфейс, размер буферов живых потоков,
//! с каких адресов браузера разрешено открывать интерфейс, откуда раздавать собранный интерфейс.
//! Значений по умолчанию нет.
//! Кто вызывает: main.rs.
//! От чего зависит: только стандартная библиотека.
//! Настройки: нет (это аргументы запуска).
//!
//! Запуск: arb --data-dir <папка> --listen <адрес:порт> --live-buffer <N> [--allow-origin <адрес> ...] [--ui-dir <папка>]

use std::net::SocketAddr;
use std::path::PathBuf;

/// Подсказка, как запускать.
const USAGE: &str = "Запуск: arb --data-dir <папка> --listen <адрес:порт> --live-buffer <N> [--allow-origin <http://адрес:порт> ...] [--ui-dir <папка собранного интерфейса>]";

/// Аргументы запуска.
#[derive(Debug)]
pub struct Args {
    /// Папка данных: база SQLite, позже зашифрованные секреты и сырые сообщения.
    pub data_dir: PathBuf,
    /// Адрес веб-сервера интерфейса.
    pub listen: SocketAddr,
    /// Сколько событий держать в памяти для живых потоков интерфейса (больше — интерфейс получит «lagged»).
    pub live_buffer: usize,
    /// Дополнительные адреса страниц браузера, которым разрешён доступ (например, сервер разработки интерфейса).
    /// Адрес самого --listen разрешён всегда.
    pub allow_origins: Vec<String>,
    /// Папка собранного интерфейса (ui/dist): задана — веб-сервер раздаёт его на «/»;
    /// не задана — работает только API (например, интерфейс идёт с сервера разработки Vite).
    pub ui_dir: Option<PathBuf>,
}

/// Разбирает аргументы.
/// Принимает: аргументы без имени программы.
/// Возвращает: Args или текст ошибки с подсказкой, как запускать.
pub fn parse(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let (mut data_dir, mut listen, mut live_buffer) = (None, None, None);
    let mut allow_origins = Vec::new();
    let mut ui_dir = None;
    while let Some(flag) = it.next() {
        let value = it
            .next()
            .ok_or_else(|| format!("После {flag} нужно значение"))?;
        match flag.as_str() {
            "--data-dir" => data_dir = Some(PathBuf::from(value)),
            "--listen" => {
                listen = Some(
                    value
                        .parse::<SocketAddr>()
                        .map_err(|e| format!("--listen {value}: {e}"))?,
                )
            }
            "--live-buffer" => {
                let n = value
                    .parse::<usize>()
                    .map_err(|e| format!("--live-buffer {value}: {e}"))?;
                if n == 0 {
                    return Err("--live-buffer должен быть больше нуля".into());
                }
                live_buffer = Some(n)
            }
            "--allow-origin" => allow_origins.push(value.trim_end_matches('/').to_string()),
            "--ui-dir" => {
                let dir = PathBuf::from(&value);
                if !dir.join("index.html").is_file() {
                    return Err(format!(
                        "--ui-dir {value}: нет index.html — соберите интерфейс (cd ui && npm ci && npm run build)"
                    ));
                }
                ui_dir = Some(dir)
            }
            other => return Err(format!("Неизвестный аргумент {other}. {USAGE}")),
        }
    }
    Ok(Args {
        data_dir: data_dir.ok_or_else(|| format!("Не указан --data-dir. {USAGE}"))?,
        listen: listen.ok_or_else(|| format!("Не указан --listen. {USAGE}"))?,
        live_buffer: live_buffer.ok_or_else(|| format!("Не указан --live-buffer. {USAGE}"))?,
        allow_origins,
        ui_dir,
    })
}
