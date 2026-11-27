//! Файл: tests/common/first_run_channels.rs
//! Что делает: О-68. Сверяет таблицу оператора с читаемыми настройками, каталогом и предзаполнением первого запуска.
//! Кто вызывает: tests/settings_cases_guard.rs в обычной матрице GitHub Actions. Чтения сети и настроек оператора нет.
//! От чего зависит: docs/FIRST_RUN.md, config/first_run.toml, config/first_run_dex.json и схема настроек.
#[path = "first_run_channel_sources.rs"]
mod sources;
use arb::{
    secrets,
    settings::{catalog, scope::Scope, value},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[path = "first_run_channel_presets.rs"]
mod preset;
use preset::{Presets, input, presets, scope};

fn check_row(
    key: &str,
    area: &str,
    advice: &str,
    source: &str,
    presets: &Presets,
) -> Result<(), String> {
    if source.trim_matches([' ', '-', '—']).is_empty() {
        return Err("не указан источник".into());
    }
    let owner = advice.strip_prefix("Задаёт владелец").is_some_and(|s| {
        s.trim_matches([' ', ':', '—', '.'])
            .chars()
            .any(char::is_alphabetic)
    });
    if secrets::KNOWN.iter().any(|&(k, _, _)| k == key) {
        return if matches!(area, "Секреты" | "Vault") && owner {
            Ok(())
        } else {
            Err("секрет требует указания владельцу в области «Секреты»".into())
        };
    }
    let def = catalog::find(key).ok_or("нет в каталоге")?;
    let s = scope(area).ok_or("неизвестная область таблицы")?;
    if s == Scope::Global && arb::settings::evm_profiles::local_only(key) {
        return Err("целевой профиль не наследует общую область BSC".into());
    }
    if !def.scopes.contains(&s.kind()) {
        return Err(format!("каталог запрещает {area}"));
    }
    if matches!(s, Scope::Dex(_)) && !presets.keys().any(|(_, id)| id == area) {
        return Err("DEX не привязан к поставляемому профилю этой сети".into());
    }
    let expected = presets.get(&(key.to_owned(), area.to_owned()));
    if owner {
        return if expected.is_none() {
            Ok(())
        } else {
            Err("для этой области есть конкретное значение первого запуска".into())
        };
    }
    if advice.starts_with("Не используется") {
        return if def.description.contains("Не используется с решения 92")
            && matches!(area, "Global" | "Venue kyberswap")
            && source.contains("src/settings/catalog/kyber.rs")
            && (source.contains("121") || advice.contains("121"))
            && (source.contains("92") || advice.contains("92"))
        {
            Ok(())
        } else {
            Err("исключение не подтверждено каталогом и решением 92/121".into())
        };
    }
    let literal = advice
        .strip_prefix('`')
        .and_then(|s| s.strip_suffix('`'))
        .ok_or("нужны значение в обратных кавычках или инструкция владельцу")?;
    let shown = if matches!(s, Scope::Dex(_)) && literal.starts_with('[') {
        serde_json::from_str(literal).map_err(|e| format!("список профиля: {e}"))?
    } else {
        input(def.kind, literal)?
    };
    let shown = value::validate(def, &s, &shown)?;
    match expected {
        Some(v) if *v == shown => Ok(()),
        Some(_) => Err("значение инструкции расходится с предзаполнением".into()),
        None => Err("нет рекомендации с этим ключом и областью в TOML или профиле".into()),
    }
}

#[test]
fn first_run_channel_settings_match_code_scopes_and_shipped_values() {
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../../config/first_run_dex.json")).unwrap();
    let required = sources::required(&profiles);
    let values = presets(&required, &profiles);
    let doc = include_str!("../../docs/FIRST_RUN.md");
    let (_, tail) = doc
        .split_once("<!-- o68:start -->")
        .expect("нет начала таблицы О-68");
    let (table, _) = tail
        .split_once("<!-- o68:end -->")
        .expect("нет конца таблицы О-68");
    let mut keys = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut errors = Vec::new();
    for line in table
        .lines()
        .map(str::trim)
        .filter(|s| s.starts_with("| `"))
    {
        let fields: Vec<_> = line.trim_matches('|').split(" | ").map(str::trim).collect();
        let [name, area, advice, source] = fields.as_slice() else {
            panic!("неверная строка таблицы: {line}")
        };
        let key = name.trim_matches('`');
        if !seen.insert((key.to_owned(), (*area).to_owned())) {
            errors.push(format!("{key} {area}: повтор"));
        }
        keys.insert(key.to_owned());
        if let Err(e) = check_row(key, area, &advice.replace("\\|", "|"), source, &values) {
            errors.push(format!("{key} {area}: {e}"));
        }
    }
    errors.extend(
        required
            .difference(&keys)
            .map(|key| format!("{key}: в таблице нет значения или инструкции владельцу")),
    );
    errors.extend(
        values
            .keys()
            .filter(|entry| !seen.contains(*entry))
            .map(|(key, area)| format!("{key} {area}: в таблице отсутствует рабочая область")),
    );
    // Локальные настройки профиля не наследуют значение другой сети даже при ручном вводе.
    for key in required
        .iter()
        .filter(|key| arb::settings::evm_profiles::local_only(key))
    {
        for venue in arb::settings::evm_profiles::PROFILES {
            let area = format!("Venue {}", venue.venue);
            if !seen.contains(&(key.clone(), area.clone())) {
                errors.push(format!(
                    "{key} {area}: нужен собственный адрес или инструкция владельцу"
                ));
            }
        }
    }
    // Решение 7 выделяет конкретные сети. Совпадающая настройка другой сети
    // не включает разрешённый рыночный канал на целевой площадке.
    for (area, channels) in [
        (
            "Venue base_dex",
            &["hanji", "ryze", "raindex", "native_rfq", "textile"][..],
        ),
        (
            "Venue eth_dex",
            &["fermi", "hashflow", "native_rfq", "pancakex"][..],
        ),
    ] {
        for channel in channels {
            let key = format!("{channel}.trading");
            if values.get(&(key.clone(), area.to_owned())) != Some(&json!(true)) {
                errors.push(format!("{key} {area}: решение 7 требует рекомендацию true"));
            }
        }
    }
    assert!(
        errors.is_empty(),
        "О-68, {} настроек из исходников и профилей:\n{}",
        required.len(),
        errors.join("\n")
    );
}

#[test]
fn wrong_scope_and_a_different_amount_cannot_satisfy_the_same_setting() {
    let name = "ryze.router_requests_per_minute";
    let values = Presets::from([((name.into(), "Global".into()), json!(100))]);
    assert!(check_row(name, "Global", "`100`", "Ryze Router", &values).is_ok());
    assert!(check_row(name, "Global", "`99`", "Ryze Router", &values).is_err());
    assert!(check_row(name, "Venue base_dex", "`100`", "Ryze Router", &values).is_err());
    assert!(check_row("zeroex.api_key", "Vault", "`ключ`", "0x API", &values).is_err());
}

#[test]
fn legacy_market_parameters_follow_their_catalog_scope() {
    let presets = Presets::new();
    let why = "Не используется после решений 92 и 121.";
    let source = "src/settings/catalog/kyber.rs";
    assert!(check_row("kyber.quote_poll_ms", "Global", why, source, &presets).is_ok());
    assert!(check_row("kyber.quote_poll_ms", "Venue kyberswap", why, source, &presets).is_err());
    assert!(check_row("kyber.assume_zero_transfer_tax", "Venue kyberswap", why, source, &presets).is_ok());
    assert!(check_row("kyber.assume_zero_transfer_tax", "Global", why, source, &presets).is_err());
    assert!(check_row("kyber.http_timeout_ms", "Global", why, source, &presets).is_err());
}
