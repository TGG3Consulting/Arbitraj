//! Файл: cli.rs
//! Что делает: разбор аргументов запуска. Здесь только то, что нужно ДО загрузки настроек:
//! где лежат данные и на каком адресе слушать интерфейс. Значений по умолчанию нет.
//! Кто вызывает: main.rs.
//! От чего зависит: только стандартная библиотека.
//! Настройки: нет (это аргументы запуска).
//!
//! Запуск: arb --data-dir <папка> --listen <адрес:порт>

use std::net::SocketAddr;
use std::path::PathBuf;

/// Аргументы запуска.
#[derive(Debug)]
pub struct Args {
    /// Папка данных: база SQLite, позже зашифрованные секреты и сырые сообщения.
    pub data_dir: PathBuf,
    /// Адрес веб-сервера интерфейса.
    pub listen: SocketAddr,
}

/// Разбирает аргументы.
/// Принимает: аргументы без имени программы.
/// Возвращает: Args или текст ошибки с подсказкой, как запускать.
pub fn parse(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut data_dir = None;
    let mut listen = None;
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
            other => return Err(format!("Неизвестный аргумент {other}")),
        }
    }
    let usage = "Запуск: arb --data-dir <папка> --listen <адрес:порт>";
    Ok(Args {
        data_dir: data_dir.ok_or_else(|| format!("Не указан --data-dir. {usage}"))?,
        listen: listen.ok_or_else(|| format!("Не указан --listen. {usage}"))?,
    })
}
