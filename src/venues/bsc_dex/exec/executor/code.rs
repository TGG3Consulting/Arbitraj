//! Файл: venues/bsc_dex/exec/executor/code.rs
//! Что делает: артефакты сборки исполнителя из репозитория (contracts/out/, собраны contracts/build.sh) — код создания
//! (развёртывание), код в сети (подставляется в симуляцию сухой сделки, сверяется с развёрнутым), слот владельца в
//! хранилище (из раскладки solc) и проверка хеша: SHA-256 каждого файла должен совпасть с записанным в
//! ArbExecutor.sha256 — иначе байткод не используется (кто-то поменял артефакт без пересборки).
//! Решение 272: версии исполнителя — v1 (contracts/out/v1/, только execute; развёрнутые до решения 272) и v2
//! (contracts/out/v2/, + executeNative; решение 395 — артефакты заморожены, не пересобираются). Решение 395 (t181): v3
//! (contracts/out/ — текущая сборка: v2 + шаги UNI_V4 / INF_CL / INF_BIN пулов внутри менеджеров Uniswap V4 и
//! PancakeSwap Infinity). Развёртывается и подставляется в симуляцию только текущая сборка (v3); код в сети сверяется со
//! всеми тремя.
//! Решение 441 (T433-F2): v4 — v3 + шаг BALANCER_V3 (contracts/ArbExecutor.sol, номер 23). Артефакты v3 заморожены в
//! contracts/out/v3/ (копия contracts/out/ на ветке t223-f2 до пересборки); contracts/out/ — текущая сборка: до
//! пересборки contracts/build.sh она побайтно равна v3 (version_of отдаёт V3), после — код v4.
//! Решение 400 (t185): приёмка сборки на живых пулах — по классам шага из записи симуляции (live_proof.rs, live_rule.rs);
//! здесь — только SHA-256 текущего bin-runtime (runtime_sha256), к которому привязана запись.
//! Кто вызывает: exec/executor/{deploy, deploy_gate, dry, live_proof}.rs, тесты.
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

/// Решение 395 (t181): код в сети версии v2 (до шагов UNI_V4 / INF_CL / INF_BIN) и его хеши — копия contracts/out/ на
/// HEAD 982e0503, не пересобираются.
const RUNTIME_V2: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/v2/ArbExecutor.bin-runtime"
));
const SHA_V2: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/v2/ArbExecutor.sha256"
));

/// Решение 441: код в сети версии v3 (до шага BALANCER_V3) и его хеши — копия contracts/out/ до пересборки v4, не
/// пересобираются.
const RUNTIME_V3: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/v3/ArbExecutor.bin-runtime"
));
const SHA_V3: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/v3/ArbExecutor.sha256"
));

/// Решение 272: код в сети версии v1 (до executeNative) и его хеши — не пересобираются.
const RUNTIME_V1: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/v1/ArbExecutor.bin-runtime"
));
const SHA_V1: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/contracts/out/v1/ArbExecutor.sha256"
));

/// Версия исполнителя по коду в сети (решение 272). Hash — ключ памяти pools/exec_callback.rs (версия, сигнатура).
/// Порядок (Ord) — порядок выпуска: V1 < V2 < V3; каждая следующая — надмножество предыдущей (решение 395).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Version {
    /// Только execute (contracts/out/v1/): нативный BNB монетой сделки не идёт.
    V1,
    /// execute + executeNative (contracts/out/v2/): нативный BNB монетой сделки идёт.
    V2,
    /// Решение 395 (t181): v2 + шаги UNI_V4 / INF_CL / INF_BIN (contracts/out/ — текущая сборка, её разворачивает кнопка
    /// «Развернуть исполнитель»).
    V3,
    /// Решение 441: v3 + шаг BALANCER_V3 (contracts/out/ — текущая сборка после пересборки contracts/build.sh).
    V4,
}

impl Version {
    /// Метка версии для записи кошелька и интерфейса: «v1» / «v2». Ошибок нет.
    pub fn label(self) -> &'static str {
        match self {
            Version::V1 => "v1",
            Version::V2 => "v2",
            Version::V3 => "v3",
            Version::V4 => "v4",
        }
    }

    /// Есть ли executeNative (решение 272): у v2 и v3. Ошибок нет.
    pub fn native(self) -> bool {
        self >= Version::V2
    }

    /// Версия по метке записи кошелька. None — метки нет (не сверена) или незнакомая. Ошибок нет.
    pub fn parse(s: &str) -> Option<Version> {
        match s {
            "v1" => Some(Version::V1),
            "v2" => Some(Version::V2),
            "v3" => Some(Version::V3),
            "v4" => Some(Version::V4),
            _ => None,
        }
    }
}

/// Версия исполнителя по коду в сети: побайтно равен коду v3 (текущая сборка), v2 или v1 из репозитория. Принимает: код
/// по адресу. Возвращает: Some(версия) или None — код не наш. Ошибки (текст): артефакт не сверен по хешу.
pub fn version_of(have: &Bytes) -> Result<Option<Version>, String> {
    // Решение 441: замороженный v3 — раньше текущей сборки: до пересборки contracts/out/ = out/v3/ (это v3, не v4).
    if *have == runtime_v3()? {
        return Ok(Some(Version::V3));
    }
    if *have == runtime()? {
        return Ok(Some(Version::V4));
    }
    if *have == runtime_v2()? {
        return Ok(Some(Version::V2));
    }
    if *have == runtime_v1()? {
        return Ok(Some(Version::V1));
    }
    Ok(None)
}

/// Решение 399 / 400: записанный SHA-256 текущего bin-runtime (ArbExecutor.sha256) — запись симуляции на живых пулах
/// относится к сборке, только если её README содержит этот SHA (exec/executor/live_rule.rs). Ошибок нет (None — строки
/// нет).
pub fn runtime_sha256() -> Option<&'static str> {
    recorded("ArbExecutor.bin-runtime")
}

/// Код в сети версии v3 (решение 441: замороженная копия). Ошибки: как checked (хеш — из contracts/out/v3/ArbExecutor.sha256).
pub fn runtime_v3() -> Result<Bytes, String> {
    checked_in(
        SHA_V3,
        "v3/ArbExecutor.bin-runtime",
        "ArbExecutor.bin-runtime",
        RUNTIME_V3,
    )
}

/// Код в сети версии v2 (решение 395: замороженная копия). Ошибки: как checked (хеш — из contracts/out/v2/ArbExecutor.sha256).
pub fn runtime_v2() -> Result<Bytes, String> {
    checked_in(
        SHA_V2,
        "v2/ArbExecutor.bin-runtime",
        "ArbExecutor.bin-runtime",
        RUNTIME_V2,
    )
}

/// Код в сети версии v1 (решение 272). Ошибки: как checked (хеш — из contracts/out/v1/ArbExecutor.sha256).
pub fn runtime_v1() -> Result<Bytes, String> {
    checked_in(
        SHA_V1,
        "v1/ArbExecutor.bin-runtime",
        "ArbExecutor.bin-runtime",
        RUNTIME_V1,
    )
}

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
    recorded_in(SHA, name)
}

/// Записанный хеш файла из данного файла хешей. Ошибок нет (None — строки нет).
fn recorded_in(sha: &'static str, name: &str) -> Option<&'static str> {
    sha.lines()
        .filter_map(|l| l.split_once("  "))
        .find(|(_, n)| *n == name)
        .map(|(h, _)| h)
}

/// Содержимое файла после сверки хеша и разбор hex. Ошибки (текст): хеш не записан или не совпал, не hex.
fn checked(name: &str, text: &str) -> Result<Bytes, String> {
    checked_in(SHA, name, name, text)
}

/// То же по данному файлу хешей. Принимает: файл хешей, путь для текста (от contracts/out/), имя в файле хешей, текст.
fn checked_in(sha: &'static str, path: &str, name: &str, text: &str) -> Result<Bytes, String> {
    let have = hex::encode(Sha256::digest(text.as_bytes()));
    match recorded_in(sha, name) {
        Some(want) if want == have => {}
        Some(want) => {
            return Err(format!(
                "contracts/out/{path}: SHA-256 {have} ≠ записанному {want} — пересоберите contracts/build.sh"
            ));
        }
        None => {
            let file = path
                .rsplit_once('/')
                .map_or("ArbExecutor.sha256".to_string(), |(d, _)| {
                    format!("{d}/ArbExecutor.sha256")
                });
            return Err(format!("contracts/out/{file}: нет хеша {name}"));
        }
    }
    hex::decode(text.trim())
        .map(Bytes::from)
        .map_err(|e| format!("contracts/out/{path}: не hex ({e})"))
}

/// Код создания исполнителя без аргументов конструктора (его SHA-256 видит человек). Ревью 272 (А1): конструктор v2
/// принимает адрес обёртки — полный код транзакции создания собирает creation_with. Ошибки: как checked.
pub fn creation() -> Result<Bytes, String> {
    checked("ArbExecutor.bin", BIN)
}

/// Код транзакции создания: код создания + аргумент конструктора constructor(address wnative_) в кодировке ABI (слово
/// 32 байта, адрес справа — Solidity ABI spec, «Formal Specification of the Encoding», address = uint160). Владелец —
/// развёртывающий кошелёк. Принимает: адрес обёртки (dex.native_wrapped). Ошибки: как checked.
pub fn creation_with(wnative: alloy_primitives::Address) -> Result<Bytes, String> {
    let mut v = creation()?.to_vec();
    v.extend_from_slice(B256::left_padding_from(wnative.as_slice()).as_slice());
    Ok(Bytes::from(v))
}

/// Код исполнителя в сети — текущая сборка (с решения 395 — v3). Ошибки: как checked.
pub fn runtime() -> Result<Bytes, String> {
    checked("ArbExecutor.bin-runtime", RUNTIME)
}

/// Слот хранилища переменной `owner` (раскладка solc; смещение в слоте должно быть 0). Ошибки (текст): раскладка не
/// сверена по хешу, не разбирается, нет `owner` или смещение не 0.
pub fn owner_slot() -> Result<B256, String> {
    slot_of("owner")
}

/// Ревью 272 (А1): слот хранилища переменной `wnative` (адрес обёртки из конструктора v2; у v1 её нет). Сухая сделка
/// подставляет его (dry.rs), сверка читает (onchain.rs). Ошибки (текст): как owner_slot.
pub fn wnative_slot() -> Result<B256, String> {
    slot_of("wnative")
}

/// Слот переменной хранилища по имени (раскладка solc v2, смещение 0). Ошибки (текст): раскладка не сверена по хешу,
/// не разбирается, нет переменной или смещение не 0.
fn slot_of(label: &str) -> Result<B256, String> {
    let have = hex::encode(Sha256::digest(LAYOUT.as_bytes()));
    if recorded("ArbExecutor.layout.json") != Some(have.as_str()) {
        return Err("contracts/out/ArbExecutor.layout.json: SHA-256 не совпал с записанным".into());
    }
    let v: serde_json::Value =
        serde_json::from_str(LAYOUT).map_err(|e| format!("раскладка хранилища: {e}"))?;
    let var = v["storage"]
        .as_array()
        .and_then(|a| a.iter().find(|s| s["label"] == label))
        .ok_or_else(|| format!("раскладка хранилища: нет переменной {label}"))?;
    if var["offset"].as_u64() != Some(0) {
        return Err(format!("раскладка хранилища: {label} не с начала слота"));
    }
    let slot: U256 = var["slot"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("раскладка хранилища: слот {label} не число"))?;
    Ok(B256::from(slot))
}
