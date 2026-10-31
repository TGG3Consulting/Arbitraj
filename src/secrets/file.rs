//! Файл: secrets/file.rs
//! Что делает: чтение и надёжная запись файла секретов secrets.enc и разбор его содержимого.
//! Запись атомарная: временный файл → сброс на диск → переименование поверх старого,
//! поэтому сбой питания посреди записи не портит файл.
//! Кто вызывает: secrets/vault.rs.
//! От чего зависит: secrets/crypto.rs, std::fs, serde_json, zeroize.
//! Настройки: нет. Путь — папка данных из аргумента --data-dir.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::crypto::{self, KdfParams};

/// Открытые секреты: имя → значение. Значения затираются в памяти при удалении.
pub type Plain = BTreeMap<String, Zeroizing<String>>;

/// Ошибка работы с файлом секретов (понятный текст для интерфейса).
pub type FileError = String;

/// Путь к файлу секретов в папке данных.
pub fn path_in(data_dir: &Path) -> PathBuf {
    data_dir.join("secrets.enc")
}

/// Читает и расшифровывает файл.
/// Принимает: путь, пароль. Возвращает: секреты.
/// Ошибки: нет файла, неверный пароль или повреждение, неизвестный формат.
pub fn load(path: &Path, password: &[u8]) -> Result<Plain, FileError> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Не удалось прочитать файл секретов: {e}"))?;
    let plain = crypto::open(password, &bytes).map_err(|e| e.to_string())?;
    let map: BTreeMap<String, String> = serde_json::from_slice(&plain)
        .map_err(|e| format!("Содержимое файла секретов повреждено: {e}"))?;
    Ok(map
        .into_iter()
        .map(|(k, v)| (k, Zeroizing::new(v)))
        .collect())
}

/// Шифрует и атомарно записывает файл.
/// Принимает: путь, пароль, секреты, параметры Argon2id.
/// Ошибки: параметры, нет места или доступа к папке.
pub fn save(path: &Path, password: &[u8], plain: &Plain, p: KdfParams) -> Result<(), FileError> {
    let as_map: BTreeMap<&str, &str> = plain
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let json = Zeroizing::new(serde_json::to_vec(&as_map).map_err(|e| e.to_string())?);
    let bytes = crypto::seal(password, &json, p).map_err(|e| e.to_string())?;
    write_blob(path, &bytes)
}

/// Читает файл секретов как есть, БЕЗ расшифровки (зашифрованный блоб для экспорта/бэкапа).
/// Принимает: путь. Возвращает: байты файла или None, если файла нет.
/// Ошибки: сбой чтения (нет доступа и т. п.).
pub fn read_blob(path: &Path) -> Result<Option<Vec<u8>>, FileError> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Не удалось прочитать файл секретов: {e}")),
    }
}

/// Атомарно записывает уже готовые байты файла секретов (временный файл → сброс → переименование).
/// Принимает: путь, байты зашифрованного файла. Шифрованием не занимается — кладёт блоб как есть.
/// Ошибки: нет места или доступа к папке.
pub fn write_blob(path: &Path, bytes: &[u8]) -> Result<(), FileError> {
    let tmp = path.with_extension("enc.tmp");
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Не удалось записать файл секретов: {e}")
    })
}
