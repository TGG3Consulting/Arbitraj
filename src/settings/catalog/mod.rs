//! Файл: settings/catalog/mod.rs
//! Что делает: полный список настроек системы — зеркало docs/SETTINGS.md. Значений нет.
//! Список разбит по файлам: base.rs (базовые), trade.rs (налоги, крючки, возврат USDT, застрявшие монеты,
//! ручной чёрный список), dex.rs (адаптер DEX и узел BSC), send.rs (отправка и каналы).
//! Кто вызывает: settings/store.rs (проверка имени и типа), api/settings.rs (схема для интерфейса).
//! От чего зависит: settings/schema.rs.
//! Настройки: это и есть их список.
//!
//! Новая настройка: строка в docs/SETTINGS.md + строка в нужном файле этой папки + поле в интерфейсе.

mod base;
mod dex;
mod send;
mod trade;

use super::schema::{Kind as K, Limit as L, ScopeKind as S, SettingDef};

/// Короткая запись одной настройки. Принимает: группу, имя, тип, ограничение, единицу, области, описание.
/// Возвращает: SettingDef без вариантов выбора и без верхней границы (их добавляют .choices() и .max()).
const fn d(
    group: &'static str,
    name: &'static str,
    kind: K,
    limit: L,
    unit: &'static str,
    scopes: &'static [S],
    description: &'static str,
) -> SettingDef {
    SettingDef {
        name,
        kind,
        limit,
        unit,
        scopes,
        description,
        group,
        choices: &[],
        max: None,
    }
}

const G: &[S] = &[S::Global];
const GV: &[S] = &[S::Global, S::Venue];
const GC: &[S] = &[S::Global, S::Coin];
const V: &[S] = &[S::Venue];
const VC: &[S] = &[S::Venue, S::Coin];
const C: &[S] = &[S::Coin];
const E: &[S] = &[S::Event];
const DX: &[S] = &[S::Dex];
const CH: &[S] = &[S::Channel];

/// Все части каталога. Порядок — как в docs/SETTINGS.md.
static PARTS: &[&[SettingDef]] = &[base::BASE, trade::TRADE, dex::DEX, send::SEND];

/// Все настройки системы по порядку docs/SETTINGS.md. Ошибок нет.
pub fn all() -> impl Iterator<Item = &'static SettingDef> {
    PARTS.iter().flat_map(|p| p.iter())
}

/// Ищет описание настройки по имени.
/// Принимает: имя. Возвращает: описание или None, если такой настройки нет.
pub fn find(name: &str) -> Option<&'static SettingDef> {
    all().find(|s| s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Имена уникальны, у выбора есть варианты, у остальных — нет.
    #[test]
    fn catalog_is_consistent() {
        let mut names: Vec<&str> = all().map(|s| s.name).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(n, names.len(), "имена настроек повторяются");
        for s in all() {
            assert_eq!(s.kind == K::Choice, !s.choices.is_empty(), "{}", s.name);
            assert!(!s.scopes.is_empty(), "{}", s.name);
        }
    }
}
