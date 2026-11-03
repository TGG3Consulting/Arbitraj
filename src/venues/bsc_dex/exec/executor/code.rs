//! Файл: venues/bsc_dex/exec/executor/code.rs
//! Что делает: артефакты сборки исполнителя из репозитория (contracts/out/, собраны contracts/build.sh) — код создания
//! (развёртывание), код в сети (подставляется в симуляцию сухой сделки, сверяется с развёрнутым), слот владельца в
//! хранилище (из раскладки solc) и проверка хеша: SHA-256 каждого файла должен совпасть с записанным в
//! ArbExecutor.sha256 — иначе байткод не используется (кто-то поменял артефакт без пересборки).
//! Кто вызывает: exec/executor/{deploy, dry}.rs, тесты.
//! От чего зависит: sha2, serde_json, contracts/out/*.
//! Настройки: нет.

use alloy_primitives::{B256, Bytes, U256};
use sha2::{Digest, Sha256};

/// Код создания (hex, как пишет solc).
const BIN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/ArbExecutor.bin"
));
/// Код в сети (deployedBytecode).
const RUNTIME: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/ArbExecutor.bin-runtime"
));
/// Раскладка хранилища (storageLayout solc).
const LAYOUT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/ArbExecutor.layout.json"
));
/// Хеши артефактов и версия компилятора (первая строка «solc …»).
const SHA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/ArbExecutor.sha256"
));

/// Версия компилятора из ArbExecutor.sha256. Ошибок нет (пусто — строки нет).
pub fn solc() -> &'static str {
    SHA.lines()
        .next()
        .and_then(|l| l.strip_prefix("solc "))
        .unwrap_or_default()
}

/// SHA-256 кода создания (hex) — показывается человеку перед развёртыванием. Ошибок нет.
pub fn creation_sha256() -> String {
    hex::encode(Sha256::digest(BIN.as_bytes()))
}

/// Записанный хеш файла из ArbExecutor.sha256. Ошибок нет (None — строки нет).
fn recorded(name: &str) -> Option<&'static str> {
    SHA.lines()
        .filter_map(|l| l.split_once("  "))
        .find(|(_, n)| *n == name)
        .map(|(h, _)| h)
}

/// Содержимое файла после сверки хеша и разбор hex. Ошибки (текст): хеш не записан или не совпал, не hex.
fn checked(name: &str, text: &str) -> Result<Bytes, String> {
    let have = hex::encode(Sha256::digest(text.as_bytes()));
    match recorded(name) {
        Some(want) if want == have => {}
        Some(want) => {
            return Err(format!(
                "contracts/out/{name}: SHA-256 {have} ≠ записанному {want} — пересоберите contracts/build.sh"
            ));
        }
        None => return Err(format!("contracts/out/ArbExecutor.sha256: нет хеша {name}")),
    }
    hex::decode(text.trim())
        .map(Bytes::from)
        .map_err(|e| format!("contracts/out/{name}: не hex ({e})"))
}

/// Код создания исполнителя (конструктор без аргументов: владелец = развёртывающий кошелёк). Ошибки: как checked.
pub fn creation() -> Result<Bytes, String> {
    checked("ArbExecutor.bin", BIN)
}

/// Код исполнителя в сети. Ошибки: как checked.
pub fn runtime() -> Result<Bytes, String> {
    checked("ArbExecutor.bin-runtime", RUNTIME)
}

/// Слот хранилища переменной `owner` (раскладка solc; смещение в слоте должно быть 0). Ошибки (текст): раскладка не
/// сверена по хешу, не разбирается, нет `owner` или смещение не 0.
pub fn owner_slot() -> Result<B256, String> {
    let have = hex::encode(Sha256::digest(LAYOUT.as_bytes()));
    if recorded("ArbExecutor.layout.json") != Some(have.as_str()) {
        return Err("contracts/out/ArbExecutor.layout.json: SHA-256 не совпал с записанным".into());
    }
    let v: serde_json::Value =
        serde_json::from_str(LAYOUT).map_err(|e| format!("раскладка хранилища: {e}"))?;
    let owner = v["storage"]
        .as_array()
        .and_then(|a| a.iter().find(|s| s["label"] == "owner"))
        .ok_or("раскладка хранилища: нет переменной owner")?;
    if owner["offset"].as_u64() != Some(0) {
        return Err("раскладка хранилища: owner не с начала слота".into());
    }
    let slot: U256 = owner["slot"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or("раскладка хранилища: слот owner не число")?;
    Ok(B256::from(slot))
}
