//! Файл: core/delist.rs
//! Что делает: делистинги бирж в памяти ядра (решение 175, Тигран 05.11.2026) — фоновое обновление по каждой бирже
//! (контракт delistings) с частотой delist.refresh_s и уведомление venue.delisting, когда на подключённой бирже в
//! делистинге появляется новая пара (первая загрузка после запуска — одна сводка). Сбой запроса — запись в журнал
//! (повтор той же ошибки — один раз), прежний список остаётся. Само решение «торговать или нет» — delist_check.rs.
//! Кто вызывает: core/engine.rs (run — одна задача на каждую биржу), core/delist_check.rs (get), api (GET
//! /api/delistings через Core::delistings), тесты (set).
//! От чего зависит: engine.rs, wait.rs, registry_sync.rs (venue.enabled), delist_text.rs, gateway (delistings).
//! Настройки: delist.refresh_s (по площадке или вся система; не задана — обновления нет, журнал core.not_configured),
//! venue.enabled.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::core::types::{Delistings, VenueId};
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::wait::{Pacer, Unit};

/// Что известно о делистингах одной биржи.
#[derive(Debug, Clone, Serialize)]
pub struct VenueDelist {
    /// Последний успешный ответ площадки (список или «неизвестно»). None — ещё ни разу не получен.
    pub got: Option<Delistings>,
    /// Когда получен последний успешный ответ.
    pub at: Option<DateTime<Utc>>,
    /// Последняя ошибка запроса (None — последний запрос успешен).
    pub error: Option<String>,
}

/// Делистинги всех бирж (поле ядра inner.delist).
#[derive(Default)]
pub struct DelistState(Mutex<HashMap<VenueId, VenueDelist>>);

impl DelistState {
    /// Ставит ответ площадки. Принимает: площадку, ответ, момент. Возвращает: прежний ответ (None — первый). Ошибок нет.
    pub fn set(&self, venue: &VenueId, got: Delistings, at: DateTime<Utc>) -> Option<Delistings> {
        let mut g = lk(&self.0);
        let e = g.entry(venue.clone()).or_insert(VenueDelist {
            got: None,
            at: None,
            error: None,
        });
        e.at = Some(at);
        e.error = None;
        e.got.replace(got)
    }

    /// Запоминает ошибку запроса (прежний ответ остаётся). Возвращает: та же ошибка уже была. Ошибок нет.
    pub fn fail(&self, venue: &VenueId, error: String) -> bool {
        let mut g = lk(&self.0);
        let e = g.entry(venue.clone()).or_insert(VenueDelist {
            got: None,
            at: None,
            error: None,
        });
        let same = e.error.as_deref() == Some(error.as_str());
        e.error = Some(error);
        same
    }

    /// Состояние одной площадки. None — делистинги этой площадки не запрашивались (DEX или задача ещё не дошла).
    pub fn get(&self, venue: &VenueId) -> Option<VenueDelist> {
        lk(&self.0).get(venue).cloned()
    }

    /// Все площадки, по id. Ошибок нет.
    pub fn all(&self) -> Vec<(VenueId, VenueDelist)> {
        let mut v: Vec<_> = lk(&self.0)
            .iter()
            .map(|(k, x)| (k.clone(), x.clone()))
            .collect();
        v.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        v
    }
}

/// Задача обновления делистингов одной биржи. Работает, пока жива система. Ошибок не возвращает: сбой — в журнал.
pub async fn run(core: Core, venue: VenueId) {
    let deps = core.inner.deps.clone();
    let mut pacer = Pacer::new("delist.refresh_s", Unit::Seconds, Some(venue.0.clone()));
    let never = tokio::sync::Notify::new();
    loop {
        if super::registry_sync::enabled(&core, &venue) {
            refresh(&core, &venue).await;
        }
        pacer.pause(&deps, &never).await;
    }
}

/// Один запрос делистингов площадки: ответ — в память и уведомление о новых парах; сбой — в журнал один раз.
/// Принимает: ядро, площадку. Ошибок нет.
pub async fn refresh(core: &Core, venue: &VenueId) {
    let deps = &core.inner.deps;
    let res = match deps.gw.get(venue).await {
        Ok(v) => v.delistings().await,
        Err(e) => Err(e),
    };
    match res {
        Ok(got) => {
            let old = core.inner.delist.set(venue, got.clone(), Utc::now());
            announce(core, venue, old.as_ref(), &got).await;
        }
        Err(e) => {
            if !core.inner.delist.fail(venue, e.message.clone()) {
                let text = format!("Делистинги не обновлены: {}", e.message);
                deps.log(Level::Warn, "core.delist", Some(&venue.0), text, None)
                    .await;
            }
        }
    }
}

/// Уведомление venue.delisting: первая загрузка — одна сводка (если есть пары или «неизвестно» — журнал), дальше — по
/// каждой новой паре. Принимает: ядро, площадку, прежний и новый ответ. Ошибок нет.
async fn announce(core: &Core, venue: &VenueId, old: Option<&Delistings>, new: &Delistings) {
    let deps = &core.inner.deps;
    if let Some(why) = &new.unknown {
        if old.is_none_or(|o| o.unknown.is_none()) {
            let text = format!(
                "Делистинги — неизвестно: {why}; сделки по этой бирже не блокируются по делистингу (решение 175)"
            );
            deps.log(Level::Info, "core.delist", Some(&venue.0), text, None)
                .await;
        }
        return;
    }
    let fresh: Vec<_> = match old {
        None => new.items.iter().collect(),
        Some(o) => {
            let was: HashSet<&str> = o.items.iter().map(|i| i.symbol.as_str()).collect();
            new.items
                .iter()
                .filter(|i| !was.contains(i.symbol.as_str()))
                .collect()
        }
    };
    if fresh.is_empty() {
        return;
    }
    let text = super::delist_text::message(&venue.0, &fresh, old.is_none());
    deps.log(
        Level::Warn,
        "core.delist",
        Some(&venue.0),
        text.clone(),
        None,
    )
    .await;
    deps.notify("venue.delisting", Some(&venue.0), &text).await;
}
