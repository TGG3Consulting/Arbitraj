//! Файл: tests/pool_pass_hot_guard.rs
//! Что делает: решение 401, ревью t186 (Н1) — сторож горячего пути котировки по исходникам (по образцу
//! tests/settings_before_send_guard.rs). 17.11 на живом боте нашли борьбу 48 потоков за замок на пути котировки; путь
//! исполнимости шага исполнителем не должен брать замки общих структур:
//! - функции пути (entry fits_ok / fits / singleton, exec_kind via_executor, exec_callback usable, pool_gate decide /
//!   admitted / admits, pool_pass kind, live_proof accepted_for и live_record find (решение 461: запись пары версия ×
//!   сеть), Ctx::exec_info, Accounts::with_wallet) не содержат `.read()`, `.write()`, `.lock()`, чтения записи
//!   кошелька с копией (`.wallet()`, `.wallets()`, `wallet_name(`) и полного факта пробы (`pool_pass::get(`);
//! - самые частые (fits_ok, decide, admitted, kind, accepted_for, find, exec_info, with_wallet, via_executor, usable) —
//!   ещё и без строк (`format!`) и копий (`.clone()`): текст причины строится лениво (pool_gate.rs refusal, entry.rs fits).
//!
//! Ревью t235: сеть площадки в pool_gate::decide — ячейка окружения Ctx::chain_id (ctx_chain.rs: загрузка OnceLock и
//! атомарная загрузка, без Store::resolve); chain_id и chain_from — в списке (без замков; ячейка создаётся один раз с
//! копией хранилища — не строгие).
//!
//! Чего сторож не ловит (docs/TASKS.md, ревью t186): вызовы из этих функций в другие. Чтение настроек каталога —
//! Store::cache после решения 409 неизменяемый снимок за ArcSwap (settings/snapshot.rs, без замка), но Store::resolve /
//! get_exact строят строку области (Scope::Venue / Scope::Dex — выделение на вызов): ctx.dex_text в
//! exec_callback::usable, как и во всех чтениях настроек котировки; память SEEN под Mutex — только у DEX с заданной
//! dex.v3_swap_callback (решение 409: SEEN — снимок без замка, замок только у записи нового селектора).
//! Кто вызывает: cargo test.
//! От чего зависит: исходники src/ (только чтение).

/// (файл, функция, строгий ли — без строк и копий).
const HOT: [(&str, &str, bool); 16] = [
    ("src/venues/bsc_dex/exec/executor/entry.rs", "fits_ok", true),
    ("src/venues/bsc_dex/exec/executor/entry.rs", "fits", false),
    (
        "src/venues/bsc_dex/exec/executor/entry.rs",
        "singleton",
        false,
    ),
    (
        "src/venues/bsc_dex/pools/exec_kind.rs",
        "via_executor",
        true,
    ),
    ("src/venues/bsc_dex/pools/exec_callback.rs", "usable", true),
    (
        "src/venues/bsc_dex/exec/executor/pool_gate.rs",
        "decide",
        true,
    ),
    (
        "src/venues/bsc_dex/exec/executor/pool_gate.rs",
        "admitted",
        true,
    ),
    (
        "src/venues/bsc_dex/exec/executor/pool_gate.rs",
        "admits",
        false,
    ),
    (
        "src/venues/bsc_dex/exec/executor/pool_pass.rs",
        "kind",
        true,
    ),
    (
        "src/venues/bsc_dex/exec/executor/live_proof.rs",
        "accepted_for",
        true,
    ),
    (
        "src/venues/bsc_dex/exec/executor/live_record.rs",
        "find",
        true,
    ),
    ("src/venues/bsc_dex/ctx_wallet.rs", "exec_info", true),
    ("src/settings/accounts/mod.rs", "with_wallet", true),
    ("src/venues/bsc_dex/ctx_wallet.rs", "executor", false),
    ("src/venues/bsc_dex/ctx_chain.rs", "chain_id", false),
    ("src/venues/bsc_dex/ctx_chain.rs", "chain_from", false),
];

/// Запрещено во всех функциях пути.
const LOCKS: [&str; 6] = [
    ".read()",
    ".write()",
    ".lock()",
    ".wallet()",
    ".wallets()",
    "wallet_name(",
];

/// Запрещено ещё и в строгих.
const STRICT: [&str; 3] = ["format!", ".clone()", "pool_pass::get("];

/// Текст файла. Ошибок нет (паника — файла нет).
fn read(rel: &str) -> String {
    std::fs::read_to_string(format!("{}/{rel}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Тело функции: от строки объявления до закрывающей скобки с тем же отступом; комментарии `//` отрезаны.
/// None — функции нет.
fn body(src: &str, name: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| {
        let t = l.trim_start();
        let decl = t.strip_prefix("pub ").unwrap_or(t);
        decl.starts_with(&format!("fn {name}(")) || decl.starts_with(&format!("fn {name}<"))
    })?;
    let indent: String = lines[start]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let close = format!("{indent}}}");
    let mut out = Vec::new();
    for l in &lines[start..] {
        let code = l.split("//").next().unwrap_or("");
        out.push(code.to_string());
        if *l == close {
            return Some(out);
        }
    }
    None
}

#[test]
fn hot_path_takes_no_shared_locks() {
    let mut bad = Vec::new();
    for (file, name, strict) in HOT {
        let src = read(file);
        let lines = body(&src, name).unwrap_or_else(|| panic!("{file}: нет функции {name}"));
        for l in &lines {
            let extra: &[&str] = if strict { &STRICT } else { &[] };
            for b in LOCKS.iter().chain(extra) {
                if l.contains(*b) {
                    bad.push(format!("{file} {name}: «{b}»: {}", l.trim()));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "ревью t186 (Н1): горячий путь без замков общих структур, строк и копий:\n{}",
        bad.join("\n")
    );
}
