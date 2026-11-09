//! Файл: settings/venue_kinds.rs
//! Что делает: решение 242, дополнение (Тигран 09.11: согласовано; вопрос 3 решения 242) — проверка пары «площадка ·
//! тип направления» (область VenueDir) при записи: хранилище (а через него API PUT /api/settings и импорт настроек,
//! api/settings_io.rs — оба пишут через Store::set) отклоняет бессмысленные пары с понятной причиной. Виды площадок
//! хранилище само не знает — их сообщает ядро при запуске (core/engine.rs Core::start: те же виды шлюза, по которым
//! правило стороны core/venue_dir.rs выбирает пару): площадки нет в шлюзе — отказ; «площадка:cex_dex / dex_cex /
//! cex_cex» у DEX — отказ (у всех трёх типов значение берётся у БИРЖИ — правило стороны: биржа → DEX — биржа покупки,
//! DEX → биржа — биржа продажи, биржа → биржа — биржа покупки; у DEX только «dex_dex» — один набор на все DEX ↔ DEX).
//! «dex_dex» с площадкой («bsc_dex:dex_dex») отклоняет уже разбор id (settings/venue_dir.rs parse_id).
//! Виды не сообщены (хранилище без ядра: утилиты, тесты хранилища) — проверка вида не делается (только разбор id и
//! типы настройки, как раньше). Очистка значения (None) не проверяется: старую неверную пару можно удалить. Загрузка
//! из базы не проверяется: уже записанные значения не теряются.
//! Кто вызывает: settings/store.rs (set — запись значения с областью VenueDir), core/engine.rs (set — при запуске).
//! От чего зависит: settings/venue_dir.rs (parse_id, DirKind), gateway/contract.rs (VenueKind).
//! Настройки: нет своих.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use crate::gateway::contract::VenueKind;

use super::venue_dir::{DirKind, parse_id};

/// Виды площадок шлюза по id. None — не сообщены (проверки вида нет).
#[derive(Clone, Default)]
pub struct VenueKinds {
    map: Arc<RwLock<Option<BTreeMap<String, VenueKind>>>>,
}

impl VenueKinds {
    /// Что делает: заменяет виды площадок (ядро при запуске). Принимает: (id площадки, вид). Ошибок нет.
    pub fn set(&self, kinds: impl IntoIterator<Item = (String, VenueKind)>) {
        *self.map.write().unwrap_or_else(|p| p.into_inner()) = Some(kinds.into_iter().collect());
    }

    /// Что делает: проверка id области VenueDir по видам площадок (см. шапку). Принимает: id области.
    /// Возвращает: Ok — пара осмысленна, id не разбирается (это решает check_scope_id) или виды не сообщены.
    /// Ошибки: текст причины — площадки нет в шлюзе; тип направления не подходит виду площадки.
    pub fn check(&self, id: &str) -> Result<(), String> {
        let Some((Some(venue), kind)) = parse_id(id) else {
            return Ok(());
        };
        let guard = self.map.read().unwrap_or_else(|p| p.into_inner());
        let Some(map) = guard.as_ref() else {
            return Ok(());
        };
        match map.get(venue) {
            Some(VenueKind::Cex) => Ok(()),
            Some(VenueKind::Dex) => Err(format!(
                "Область «{id}»: «{venue}» — DEX, а тип «{}» задаётся у биржи (значение берётся у биржи направления); \
                 у DEX — только «{}» (один набор на все сделки DEX ↔ DEX)",
                kind.key(),
                DirKind::DexDex.key()
            )),
            None => {
                let known: Vec<&str> = map.keys().map(String::as_str).collect();
                Err(format!(
                    "Область «{id}»: площадки «{venue}» нет в шлюзе (подключены: {})",
                    if known.is_empty() {
                        "нет".to_string()
                    } else {
                        known.join(", ")
                    }
                ))
            }
        }
    }
}
