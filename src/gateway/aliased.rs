//! Файл: gateway/aliased.rs
//! Что делает: решение 200 — обёртка адаптера БИРЖИ для склейки тикеров одного адреса (coin.ticker_alias): ядро видит
//! основной тикер, биржа в запросах получает свой (gateway/aliased_tr.rs). Карта переименований биржи строится из её
//! живого справочника сетей при каждом coin_networks (gateway/alias_map.rs) и пишет в журнал изменения и конфликты.
//! Карта пуста (склейки у биржи нет) — все вызовы идут в адаптер как есть, события рынка — напрямую в канал ядра;
//! есть склейка — события рынка идут через задачу перевода тикеров (одна на подписку, кончается вместе с ней).
//! Контракт Venue обёртки (перевод в каждом вызове) — gateway/aliased_venue.rs.
//! Кто вызывает: gateway/registry.rs (register оборачивает биржи, если реестр создан с with_aliases).
//! От чего зависит: gateway/contract.rs, alias_map.rs, aliased_tr.rs, settings, storage/journal.
//! Ошибка «не задана venue.bsc_network» из coin_networks — с подсказкой имён сетей (gateway/network_hint.rs).
//! Решение 206: справочник каждой биржи пишется в общую книгу (gateway/alias_auto.rs); при coin.ticker_alias_auto = да
//! к ручным группам добавляются автоматические — все адреса, у которых биржи зовут монету по-разному, кроме адресов
//! ручной склейки и coin.ticker_alias_off.
//! Настройки: coin.ticker_alias, coin.ticker_alias_auto, coin.ticker_alias_off (глобальные, необязательные).

use std::sync::{Arc, Mutex, RwLock};

use crate::core::types::CoinNetworkInfo;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};

use super::alias_auto::AutoBook;
use super::alias_map::{self, Group, VenueMap};
use super::contract::Venue;

/// Имя настройки склейки.
pub const SETTING: &str = "coin.ticker_alias";
/// Решение 206: автоматическая склейка всех тикеров одного адреса (да / нет).
pub const AUTO: &str = "coin.ticker_alias_auto";
/// Решение 206: адреса, которые не склеивать («Разделить»).
pub const OFF: &str = "coin.ticker_alias_off";

/// Все настройки склейки (их изменение — перечитать справочник сетей бирж, core/rebuild.rs).
pub const SETTINGS: [&str; 3] = [SETTING, AUTO, OFF];

/// Обёртка биржи.
pub struct Aliased {
    pub(super) inner: Arc<dyn Venue>,
    settings: Store,
    journal: Journal,
    map: RwLock<VenueMap>,
    /// Последний записанный в журнал текст состояния склейки (повтор не пишется).
    told: Mutex<String>,
    /// Подсказка имён сетей к ошибке «не задана venue.bsc_network» (gateway/network_hint.rs): считается один раз, пока
    /// ошибка держится (справочник не качается на каждом опросе); справочник получен — сбрасывается.
    pub(super) bsc_hint: Mutex<Option<String>>,
    /// Решение 206: общая книга тикеров по адресам (одна на реестр).
    book: Arc<Mutex<AutoBook>>,
}

impl Aliased {
    /// Оборачивает адаптер. Ошибок нет.
    pub fn new(
        inner: Arc<dyn Venue>,
        settings: Store,
        journal: Journal,
        book: Arc<Mutex<AutoBook>>,
    ) -> Self {
        Self {
            inner,
            settings,
            journal,
            map: RwLock::new(VenueMap::default()),
            told: Mutex::new(String::new()),
            bsc_hint: Mutex::new(None),
            book,
        }
    }

    /// Текущая карта (копия — она маленькая). Ошибок нет.
    pub(super) fn m(&self) -> VenueMap {
        self.map.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Ошибка справочника сетей: «не задана venue.bsc_network» — с подсказкой имён сетей из живого справочника
    /// (один раз, дальше из памяти); другая ошибка — как есть. Ошибок нет.
    pub(super) async fn with_hint(
        &self,
        mut e: super::errors::GatewayError,
    ) -> super::errors::GatewayError {
        if !super::network_hint::bsc_unset(&e) {
            return e;
        }
        let known = self
            .bsc_hint
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let hint = match known {
            Some(h) => h,
            None => {
                let h = super::network_hint::text(self.inner.as_ref(), &self.settings).await;
                *self.bsc_hint.lock().unwrap_or_else(|p| p.into_inner()) = Some(h.clone());
                h
            }
        };
        e.message = format!("{} — {hint}", e.message);
        e
    }

    /// Пересчитывает карту по живому справочнику биржи и пишет изменения в журнал. Ошибок нет.
    pub(super) async fn refresh(&self, raw: &[CoinNetworkInfo]) {
        let items = self
            .settings
            .list(SETTING, At::default())
            .unwrap_or_default();
        let (manual, mut bad) = alias_map::parse(&items);
        let groups = all_groups(
            &self.settings,
            &self.book,
            &self.inner.id().0,
            raw,
            manual,
            &mut bad,
        );
        let (map, conflicts) = alias_map::venue_map(&groups, raw);
        let mut lines: Vec<String> = map
            .renames
            .iter()
            .map(|r| format!("{} → {} (адрес {})", r.venue, r.main, r.address))
            .collect();
        lines.extend(conflicts.iter().map(|c| format!("не склеено: {c}")));
        lines.extend(
            bad.iter()
                .map(|b| format!("неверный элемент {SETTING}: {b}")),
        );
        *self.map.write().unwrap_or_else(|e| e.into_inner()) = map;
        let text = lines.join("; ");
        let changed = {
            let mut t = self.told.lock().unwrap_or_else(|e| e.into_inner());
            let c = *t != text;
            *t = text.clone();
            c
        };
        if changed && !text.is_empty() {
            let level = if conflicts.is_empty() && bad.is_empty() {
                Level::Info
            } else {
                Level::Warn
            };
            let msg = format!("Склейка тикеров (решение 200): {text}");
            let v = self.inner.id();
            let _ = self
                .journal
                .write(level, "venue.ticker_alias", Some(&v.0), msg, None)
                .await;
        }
    }
}

/// Решение 206: группы склейки — ручные (coin.ticker_alias) и, при coin.ticker_alias_auto = да, автоматические из
/// общей книги (справочник этой биржи пишется в книгу здесь же). Адреса ручных групп и coin.ticker_alias_off
/// автоматически не склеиваются; неверный адрес в coin.ticker_alias_off — в `bad`. Ошибок нет.
pub fn all_groups(
    settings: &Store,
    book: &Mutex<AutoBook>,
    venue: &str,
    raw: &[CoinNetworkInfo],
    mut groups: Vec<Group>,
    bad: &mut Vec<String>,
) -> Vec<Group> {
    let mut b = book.lock().unwrap_or_else(|e| e.into_inner());
    b.record(venue, raw);
    if !matches!(settings.flag(AUTO, At::default()), Ok(true)) {
        return groups;
    }
    let mut skip: Vec<alloy_primitives::Address> = groups.iter().map(|g| g.address).collect();
    for x in settings.list(OFF, At::default()).unwrap_or_default() {
        match x.trim().parse() {
            Ok(a) => skip.push(a),
            Err(_) => bad.push(format!("«{x}» в {OFF} — не адрес контракта")),
        }
    }
    groups.extend(b.groups(&skip));
    groups
}
