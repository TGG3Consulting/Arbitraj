//! Файл: venues/bsc_dex/dex_venues.rs
//! Что делает: решение 435 (задача 433-к) — список площадок DEX процесса: всегда bsc_dex (BSC_ID) и площадки из
//! настройки «вся система» dex.venues (например arb_dex — площадка DEX сети Arbitrum тем же кодом адаптера, решение
//! 433). Не задана или пуста — только bsc_dex, процесс как до решения 435. Читается при запуске процесса (main.rs):
//! площадка регистрируется в шлюзе до запуска ядра, смена списка вступает в силу после перезапуска.
//! Кто вызывает: main.rs.
//! От чего зависит: settings (store, read), ctx_place.rs (BSC_ID).
//! Настройки: dex.venues (вся система).

use crate::settings::scope::Scope;
use crate::settings::store::Store;

use super::ctx_place::BSC_ID;

/// Имя настройки списка площадок DEX.
pub const SETTING: &str = "dex.venues";

/// Возвращает пригодные идентификаторы и причины отказов по отдельным записям.
pub fn extra(s: &Store) -> (Vec<String>, Vec<String>) {
    let Some(value) = s.get_exact(SETTING, &Scope::Global) else {
        return (vec![], vec![]);
    };
    let Some(list) = value.as_array() else {
        return (vec![], vec![format!("{SETTING}: ожидается список id площадок DEX")]);
    };
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for v in list {
        let Some(id) = v.as_str().map(str::trim) else {
            errors.push(format!("{SETTING}: id площадки должен быть строкой"));
            continue;
        };
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
            errors.push(format!("{SETTING}: недопустимый id «{id}»"));
        } else if id == BSC_ID || out.iter().any(|existing| existing == id) {
            errors.push(format!("{SETTING}: повтор площадки «{id}»"));
        } else if let Some(p) = crate::settings::evm_profiles::profile(id)
            && let Err(why) = p.ready(s)
        {
            errors.push(why);
        } else {
            out.push(id.to_string());
        }
    }
    (out, errors)
}

/// Прежняя площадка с данными учёта сохраняет своё имя при конфликте сети или chainId с новым профилем.
pub fn unique_profiles(s: &Store, ids: &mut Vec<String>) -> Vec<String> {
    let mut errors = Vec::new();
    let mut conflicts = Vec::new();
    for p in crate::settings::evm_profiles::PROFILES {
        if !ids.iter().any(|id| id == p.venue) {
            continue;
        }
        for id in std::iter::once(BSC_ID).chain(ids.iter().map(String::as_str)) {
            if id == p.venue { continue; }
            let scope = Scope::Venue(id.to_string());
            if ["dex.chain", "dex.evm_chain_id"].iter().any(|name| p.overlaps(name, s.get_exact(name, &scope).as_ref())) {
                errors.push(format!("{}: сеть {} или chainId {} уже принадлежит {id}", p.venue, p.chain, p.evm_chain_id));
                conflicts.push(p.venue);
                break;
            }
        }
    }
    ids.retain(|id| !conflicts.contains(&id.as_str()));
    errors
}

/// Что делает: id всех площадок DEX процесса — bsc_dex первой, затем `extra`; строки живут весь процесс (Box::leak —
/// один раз при запуске). Принимает: id сверх bsc_dex. Возвращает: срез id. Ошибок нет.
pub fn all(extra: Vec<String>) -> &'static [&'static str] {
    let mut ids: Vec<&'static str> = vec![BSC_ID];
    for s in extra {
        let id: &'static str = Box::leak(s.into_boxed_str());
        ids.push(id);
    }
    Box::leak(ids.into_boxed_slice())
}
