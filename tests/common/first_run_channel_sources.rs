//! Файл: tests/common/first_run_channel_sources.rs
//! Что делает: выводит перечень настроек О-68 из каталога, рабочих модулей и поставляемых профилей.
//! Кто вызывает: tests/settings_cases_guard.rs. От чего зависит: исходники адаптеров, каталог, JSON профилей.
//! Ниже перечислены семейства решения 7 и О-68, а не имена их параметров.
use arb::{secrets, settings::catalog};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path};

const FAMILIES: &[&str] = &[
    "airswap",
    "bopamm",
    "dexalot",
    "epsilon",
    "fermi",
    "gladius",
    "hanji",
    "hashflow",
    "http",
    "kipseli",
    "kyber",
    "native_rfq",
    "pancakex",
    "raindex",
    "ryze",
    "textile",
    "tokenlon",
    "zeroex",
];
const SHARED_MODULES: &[&str] = &[
    "ambient",
    "keyed_price",
    "kyberswap",
    "launch_keys",
    "prop_venue",
    "spectra",
    "umia",
    "stelaxis",
    "homelander",
    "ondo",
];

pub(super) fn selected_profile(row: &Value) -> bool {
    let id = row["id"].as_str().unwrap();
    let kind = row["settings"]["dex.kind"].as_str().unwrap_or_default();
    match row["network"].as_str() {
        Some("base") => kind.starts_with("spectra_") || id == "umia-base",
        Some("ethereum") => kind == "ambient_ethereum" || id == "kipseli-lambda-ethereum",
        _ => false,
    }
}
fn family(name: &str) -> bool {
    name.split_once('.')
        .is_some_and(|(ns, _)| FAMILIES.contains(&ns))
}
fn related(path: &Path) -> bool {
    // Фактическое разрешение/остаток читаются через общий RPC. В native_rfq/ryze
    // и других источниках сами имена параметров транспорта не повторяются.
    if [
        "bsc_dex/node/rpc.rs",
        "bsc_dex/node/failover.rs",
        "bsc_dex/node/pace.rs",
        "bsc_dex/tokens.rs",
        "gateway/market_http.rs",
        "bsc_dex/pools/http_wait.rs",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
    {
        return true;
    }
    path.with_extension("")
        .iter()
        .filter_map(|p| p.to_str())
        .any(|p| {
            FAMILIES.iter().chain(SHARED_MODULES).any(|&m| {
                p == m
                    || (m != "http" && p.strip_prefix(m).is_some_and(|tail| tail.starts_with('_')))
            })
        })
}

/// Считываем также общие ключи в дочерних функциях, массивах обязательных полей и константах.
/// Строка должна буквально совпасть с зарегистрированным параметром/секретом; URL и диагностические метки пропускаются.
fn scan(dir: &Path, known: &BTreeSet<&str>, out: &mut BTreeSet<String>) {
    for e in fs::read_dir(dir).unwrap() {
        let path = e.unwrap().path();
        if path.is_dir() {
            scan(&path, known, out);
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if path.extension().is_none_or(|x| x != "rs")
            || !related(&path)
            || stem.contains("test")
            || stem.contains("fixture")
        {
            continue;
        }
        collect_literals(&fs::read_to_string(path).unwrap(), known, out);
    }
}

pub(super) fn required(profiles: &[Value]) -> BTreeSet<String> {
    let known: BTreeSet<_> = catalog::all()
        .map(|d| d.name)
        .chain(secrets::KNOWN.iter().map(|&(k, _, _)| k))
        .collect();
    let mut out = known
        .iter()
        .filter(|k| family(k))
        .map(|s| (*s).to_owned())
        .collect();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    scan(&src.join("venues/bsc_dex"), &known, &mut out);
    scan(&src.join("venues/kyberswap"), &known, &mut out);
    scan(&src.join("gateway"), &known, &mut out);
    for p in profiles.iter().filter(|p| selected_profile(p)) {
        out.extend(p["settings"].as_object().unwrap().keys().cloned());
    }
    out
}

fn collect_literals(text: &str, known: &BTreeSet<&str>, out: &mut BTreeSet<String>) {
    for line in text.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        if line.trim_start().starts_with("mod tests {") {
            break;
        }
        for lit in line.split('"').skip(1).step_by(2) {
            if known.contains(lit) {
                out.insert(lit.to_owned());
            }
        }
    }
}

#[test]
fn new_shared_setting_used_by_a_child_module_is_required_without_adding_a_family() {
    assert!(related(Path::new("ryze/new_child.rs")));
    assert!(!related(Path::new("gateway/http_bind.rs")));
    let known = BTreeSet::from(["shared.next_request_timeout", "shared.example"]);
    let mut found = BTreeSet::new();
    collect_literals(
        r#"
        //! Исторический пример "shared.example".
        let timeout = settings.integer(
            "shared.next_request_timeout",
            at,
        );
    "#,
        &known,
        &mut found,
    );
    assert_eq!(
        found,
        BTreeSet::from(["shared.next_request_timeout".to_owned()])
    );
}
