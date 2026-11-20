//! Файл: venues/bsc_dex/exec/executor/live_record.rs
//! Что делает: решение 461 (T457-02) — записи приёмки исполнителя на живых пулах, вшитые в сборку (include_str), по ключу
//! (версия кода, chainId сети). Кошелёк судится только записью своей версии и своей сети (live_proof.rs); записи для
//! пары нет — все классы «не проверен». Записи:
//! - v3, chainId 56 — tests/data/executor_v3_live_2026-11-17 (свой узел BSC; строки chainId в её README нет — сеть
//!   задана здесь по факту README: «WBNB (dex.native_wrapped, bsc_dex)», адреса BSC, решение 400);
//! - v4, chainId из строки README «- chainId: 42161» — tests/data/executor_v4_live_2026-11-20 (свой узел Arbitrum One,
//!   tools/arbitrum_record/exec_v4_live.py, решение 459); поле chain_id строк summary.json должно с ней совпасть.
//!
//! Вердикты — правило live_rule.rs (SHA-256 bin-runtime той версии — code::runtime_sha256_of), один разбор за запуск.
//! Записи для пары нет — missing: все классы «не проверен» (текст с парой — pair).
//! Новая запись (новая версия или сеть) — новая строка SOURCES и папка tests/data в том же коммите.
//! Кто вызывает: exec/executor/live_proof.rs, тесты.
//! От чего зависит: exec/executor/{code, live_rule}.rs, serde_json.
//! Настройки: нет (факт о собранном коде и записи, решения 400, 461).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use super::code::{self, Version};
use super::live_rule::{self, CLASSES, Verdict};

/// Сеть записи v3 2026-11-17 — chainId BSC (в её README строки chainId нет; запись — со своего узла BSC, README:
/// «WBNB (dex.native_wrapped, bsc_dex): 0xbb4c…», адреса BSC). Факт записи, не настройка.
const V3_RECORD_CHAIN: u64 = 56;

/// Вшитая запись: версия кода, каталог, тексты, сеть (Some — задана здесь, None — из строки README «- chainId: N»).
struct Source {
    version: Version,
    dir: &'static str,
    readme: &'static str,
    summary: &'static str,
    reject: &'static str,
    chain: Option<u64>,
}

const SOURCES: [Source; 2] = [
    Source {
        version: Version::V3,
        dir: "tests/data/executor_v3_live_2026-11-17",
        readme: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/executor_v3_live_2026-11-17/README.md"
        )),
        summary: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/executor_v3_live_2026-11-17/summary.json"
        )),
        reject: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/executor_v3_live_2026-11-17/hooks_reject.json"
        )),
        chain: Some(V3_RECORD_CHAIN),
    },
    Source {
        version: Version::V4,
        dir: "tests/data/executor_v4_live_2026-11-20",
        readme: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/executor_v4_live_2026-11-20/README.md"
        )),
        summary: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/executor_v4_live_2026-11-20/summary.json"
        )),
        reject: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/executor_v4_live_2026-11-20/hooks_reject.json"
        )),
        chain: None,
    },
];

/// Вердикты записи по классам (порядок live_rule::CLASSES); Err — запись не разобрана.
pub type Verdicts = Result<Vec<Verdict>, String>;

/// Разобранная запись.
pub struct Judged {
    pub version: Version,
    /// Каталог записи (для текстов).
    pub dir: &'static str,
    /// chainId сети записи; Err — сеть из записи не прочитана.
    pub chain: Result<u64, String>,
    /// Вердикты по классам (порядок live_rule::CLASSES); Err — запись не разобрана.
    pub verdicts: Verdicts,
}

/// Сеть записи по README и summary.json: строка README «- chainId: N» (ровно одна) и поле chain_id строк summary.json
/// (у каждой строки, где оно есть, — то же N). Принимает: тексты README и summary.json. Возвращает: chainId. Ошибки
/// (текст): строки нет или их несколько, число не читается, summary.json не JSON, chain_id строки другой.
pub fn chain_in(readme: &str, summary: &str) -> Result<u64, String> {
    const MARK: &str = "- chainId:";
    let found: Vec<&str> = readme
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix(MARK))
        .map(|rest| rest.split_whitespace().next().unwrap_or_default())
        .collect();
    let [one] = found.as_slice() else {
        return Err(format!(
            "в README записи строк «{MARK} N» {} — нужна ровно одна",
            found.len()
        ));
    };
    let chain: u64 = one
        .parse()
        .map_err(|e| format!("README: chainId «{one}»: {e}"))?;
    let rows: Vec<Value> =
        serde_json::from_str(summary).map_err(|e| format!("summary.json: {e}"))?;
    for r in &rows {
        let c = &r["chain_id"];
        if !c.is_null() && c.as_u64() != Some(chain) {
            return Err(format!("summary.json: chain_id строки {c} ≠ chainId README {chain}"));
        }
    }
    Ok(chain)
}

/// Все вшитые записи, разобранные один раз за запуск. Ошибок нет (ошибки — в полях записи).
pub fn all() -> &'static [Judged] {
    static ALL: OnceLock<Vec<Judged>> = OnceLock::new();
    ALL.get_or_init(|| {
        SOURCES
            .iter()
            .map(|s| {
                let chain = match s.chain {
                    Some(c) => Ok(c),
                    None => chain_in(s.readme, s.summary),
                };
                let verdicts = match &chain {
                    Err(e) => Err(format!("сеть записи не прочитана — {e}")),
                    Ok(_) => live_rule::judge(
                        s.readme,
                        s.summary,
                        s.reject,
                        code::runtime_sha256_of(s.version),
                    ),
                };
                Judged {
                    version: s.version,
                    dir: s.dir,
                    chain,
                    verdicts,
                }
            })
            .collect()
    })
}

/// Запись для пары (версия, сеть). Принимает: версию кода, chainId сети кошелька (None — сеть площадки не задана).
/// Возвращает: запись или None — записи для пары нет. Ошибок нет.
pub fn find(v: Version, chain: Option<u64>) -> Option<&'static Judged> {
    let c = chain?;
    all()
        .iter()
        .find(|j| j.version == v && j.chain.as_ref().ok() == Some(&c))
}

/// Запись этой версии, чья сеть не прочитана (её ошибка видна вместо «записи нет»). Ошибок нет.
pub fn broken(v: Version) -> Option<&'static Judged> {
    all().iter().find(|j| j.version == v && j.chain.is_err())
}

/// Имя пары для текстов: «v4 в сети chainId 42161» (сеть не задана — «в сети, где dex.evm_chain_id не задана»).
/// Ошибок нет.
pub fn pair(v: Version, chain: Option<u64>) -> String {
    match chain {
        Some(c) => format!("{} в сети chainId {c}", v.label()),
        None => format!("{} в сети, где dex.evm_chain_id не задана", v.label()),
    }
}

/// «Не проверен» по всем классам для пары без записи — один список на пару за запуск (пар — единицы: версии × сети
/// площадок). Ошибок нет.
pub fn missing(v: Version, chain: Option<u64>) -> &'static Verdicts {
    type Cache = HashMap<(Version, Option<u64>), &'static Verdicts>;
    static SEEN: Mutex<Option<Cache>> = Mutex::new(None);
    let mut g = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    let cache = g.get_or_insert_with(HashMap::new);
    if let Some(&r) = cache.get(&(v, chain)) {
        return r;
    }
    let why = format!(
        "записи приёмки для версии {} нет — не проверен",
        pair(v, chain)
    );
    let list: Vec<Verdict> = CLASSES
        .iter()
        .map(|c| Verdict {
            class: *c,
            pools: 0,
            proven: 0,
            accepted: false,
            why: why.clone(),
        })
        .collect();
    let r: &'static Verdicts = Box::leak(Box::new(Ok(list)));
    cache.insert((v, chain), r);
    r
}
