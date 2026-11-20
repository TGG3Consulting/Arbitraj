//! Файл: venues/bsc_dex/dex_venues.rs
//! Что делает: решение 435 (задача 433-к) — список площадок DEX процесса: всегда bsc_dex (BSC_ID) и площадки из
//! настройки «вся система» dex.venues (например arb_dex — площадка DEX сети Arbitrum тем же кодом адаптера, решение
//! 433). Не задана или пуста — только bsc_dex, процесс как до решения 435. Читается при запуске процесса (main.rs):
//! площадка регистрируется в шлюзе до запуска ядра, смена списка вступает в силу после перезапуска.
//! Кто вызывает: main.rs.
//! От чего зависит: settings (store, read), ctx_place.rs (BSC_ID).
//! Настройки: dex.venues (вся система).

use crate::settings::read::At;
use crate::settings::scope::Scope;
use crate::settings::store::Store;

use super::ctx_place::BSC_ID;

/// Имя настройки списка площадок DEX.
pub const SETTING: &str = "dex.venues";

/// Что делает: id площадок DEX сверх bsc_dex из dex.venues. Принимает: хранилище настроек.
/// Возвращает: id по порядку (не задана — пусто). Ошибка — текст: не список строк; пустой id; символ не из строчной
/// латиницы, цифр и «_»; id = bsc_dex; повтор.
pub fn extra(s: &Store) -> Result<Vec<String>, String> {
    if s.get_exact(SETTING, &Scope::Global).is_none() {
        return Ok(Vec::new());
    }
    let list = s
        .list(SETTING, At::default())
        .map_err(|e| format!("{SETTING}: {}", e.message))?;
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    for id in list {
        let id = id.trim().to_string();
        if id.is_empty() {
            return Err(format!("{SETTING}: пустой id площадки"));
        }
        if let Some(c) = id
            .chars()
            .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_'))
        {
            return Err(format!(
                "{SETTING}: id «{id}» — недопустимый символ «{c}» (строчная латиница, цифры, «_»)"
            ));
        }
        if id == BSC_ID {
            return Err(format!(
                "{SETTING}: {BSC_ID} есть всегда — в списке только другие площадки DEX"
            ));
        }
        if out.contains(&id) {
            return Err(format!("{SETTING}: id «{id}» повторяется"));
        }
        out.push(id);
    }
    Ok(out)
}

/// Что делает: id всех площадок DEX процесса — bsc_dex первой, затем `extra`; строки живут весь процесс (Box::leak —
/// один раз при запуске). Принимает: id сверх bsc_dex. Возвращает: срез id. Ошибок нет.
pub fn all(extra: Vec<String>) -> &'static [&'static str] {
    let mut ids: Vec<&'static str> = vec![BSC_ID];
    ids.extend(extra.into_iter().map(|s| &*Box::leak(s.into_boxed_str())));
    Box::leak(ids.into_boxed_slice())
}
