//! Файл: core/signal_ripen.rs
//! Что делает: решение 318 — таймер дозревания сигнала. Направление в «держится» (Holding) при signal.hold_timer = да
//! получает срок ripen_at = max(above_since + signal.min_hold_ms, сейчас) (signal.rs). Если до срока по монете не
//! было пересчёта, таймер сам ставит пересчёт монеты в поток тика (tick.rs schedule_set) — повторная котировка обеих
//! сторон по текущим данным; она засчитывается как обновление (updates + 1) и подтверждает, что спред ещё есть.
//! Не больше одной спящей задачи на монету (armed — срок, на который она спит); тики между — как обычно сливаются
//! (решение 109, signal.eval_min_interval_ms соблюдается: таймер только добавляет площадку в пересчёт).
//! Свежесть: пересчёт по таймеру идёт тем же путём prices.rs → котировки шлюза, которые сами отказывают на
//! устаревших данных (feed.stale_after_ms, память котировок DEX — не старше signal.dex_quote_cache_ms) — устаревший
//! стакан таймер не оживляет: «нет цены» → счётчики сбрасываются, кандидата нет.
//! Кто вызывает: core/evaluate.rs (note, take_fired), core/tick.rs (arm после каждого пересчёта монеты).
//! От чего зависит: core/engine.rs (Inner.ripen), core/tick.rs (schedule_set), core/signal.rs (SpreadView).
//! Настройки: signal.hold_timer (читает signal.rs Params; не задана — сроков нет, таймер не ставится).

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::core::types::{Asset, Direction, VenueId};

use super::engine::{Core, lk};

/// Сроки дозревания по монетам и направлениям, спящие задачи и сработавшие направления.
#[derive(Default)]
pub struct Plan {
    /// Монета → направление → (срок, площадка, которую поставить в пересчёт).
    due: HashMap<Asset, HashMap<Direction, (DateTime<Utc>, VenueId)>>,
    /// Монета → срок, на который спит её задача таймера.
    armed: HashMap<Asset, DateTime<Utc>>,
    /// Направления, чей пересчёт поставил таймер (пометка «таймер» в signal_ctx.rs).
    fired: HashSet<Direction>,
}

impl Plan {
    /// Что делает: ставит или снимает срок направления. Принимает: направление, срок (None — снять), площадку
    /// для пересчёта. Ошибок нет.
    pub fn set(&mut self, dir: &Direction, at: Option<DateTime<Utc>>, venue: &VenueId) {
        match at {
            Some(at) => {
                self.due
                    .entry(dir.asset.clone())
                    .or_default()
                    .insert(dir.clone(), (at, venue.clone()));
            }
            None => {
                if let Some(m) = self.due.get_mut(&dir.asset) {
                    m.remove(dir);
                    if m.is_empty() {
                        self.due.remove(&dir.asset);
                    }
                }
            }
        }
    }

    /// Что делает: решает, нужна ли новая спящая задача монеты. Возвращает: срок, на который её ставить (и отмечает
    /// его в armed); None — сроков нет или уже спит задача на срок не позже ближайшего. Ошибок нет.
    pub fn want_arm(&mut self, asset: &Asset) -> Option<DateTime<Utc>> {
        let earliest = self.due.get(asset)?.values().map(|(at, _)| *at).min()?;
        if self.armed.get(asset).is_some_and(|a| *a <= earliest) {
            return None;
        }
        self.armed.insert(asset.clone(), earliest);
        Some(earliest)
    }

    /// Что делает: задача, спавшая до срока `armed_at`, проснулась. Принимает: монету, свой срок, время.
    /// Возвращает: None — задача уже не текущая (её сменила задача на более ранний срок), ничего не делать;
    /// Some(площадки) — площадки направлений с наступившим сроком (сроки сняты, направления помечены «таймер»);
    /// пусто — наступивших нет (вызывающий ставит задачу заново). Ошибок нет.
    pub fn fire(
        &mut self,
        asset: &Asset,
        armed_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Option<HashSet<VenueId>> {
        if self.armed.get(asset) != Some(&armed_at) {
            return None;
        }
        self.armed.remove(asset);
        let mut venues = HashSet::new();
        if let Some(m) = self.due.get_mut(asset) {
            let ripe: Vec<Direction> = m
                .iter()
                .filter(|(_, (at, _))| *at <= now)
                .map(|(d, _)| d.clone())
                .collect();
            for d in ripe {
                if let Some((_, v)) = m.remove(&d) {
                    venues.insert(v);
                }
                self.fired.insert(d);
            }
            if m.is_empty() {
                self.due.remove(asset);
            }
        }
        Some(venues)
    }

    /// Что делает: снимает пометку «пересчёт поставил таймер». Возвращает: была ли пометка. Ошибок нет.
    pub fn take_fired(&mut self, dir: &Direction) -> bool {
        self.fired.remove(dir)
    }
}

/// Что делает: после пересчёта монеты ставит спящую задачу таймера на ближайший срок (если нужна).
/// Принимает: ядро, монету. Ошибок нет. Без сроков (signal.hold_timer не задана) — одно короткое чтение под замком.
pub fn arm(core: &Core, asset: &Asset) {
    let Some(at) = lk(&core.inner.ripen).want_arm(asset) else {
        return;
    };
    let core = core.clone();
    let asset = asset.clone();
    tokio::spawn(async move {
        let wait = (at - Utc::now()).to_std().unwrap_or_default();
        tokio::time::sleep(wait).await;
        let fired = lk(&core.inner.ripen).fire(&asset, at, Utc::now());
        match fired {
            None => {}
            Some(v) if v.is_empty() => arm(&core, &asset),
            Some(v) => super::tick::schedule_set(&core, asset, v),
        }
    });
}
