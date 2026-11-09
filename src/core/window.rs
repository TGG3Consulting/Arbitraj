//! Файл: core/window.rs
//! Что делает: окна (решение 125) — чистая логика без диска и сети. Окно монеты открывается, когда направление этой
//! монеты (монета + площадка покупки + площадка продажи) прошло расчёт прибыльности (profit.rs: Verdict::Accept —
//! пороги money.min_profit_usd / pct пройдены; проверки денег и замков исполнителя — уже после). Пока окно открыто,
//! другие направления той же монеты (тот же адрес контракта, CoinKey) входят в него же — окна одной монеты, которые
//! перекрылись бы по времени, — одно окно; площадки новых направлений добавляются к записи. Направление перестало
//! проходить — убирается из «проходящих»; когда не проходит ни одно, окно ждёт window.min_hold_ms (прошло снова —
//! то же окно) и закрывается, если по монете нет открытой сделки; сделка идёт — окно ждёт её конца (что позже).
//! Решение 241: у открытого окна — памятка последних точек цены пула DEX по направлениям (dex_slot, window_dex.rs).
//! Кто вызывает: core/window_run.rs (pass, fail), core/window_worker.rs (closable, close, deadline), core/window_dex.rs
//! (dex_slot), тесты.
//! От чего зависит: core/types, coin_key.rs, window_info.rs (сводка окна), chrono, rust_decimal.
//! Настройки: нет (min_hold_ms передаёт вызывающий из window.min_hold_ms).

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{Direction, VenueId};

use super::coin_key::CoinKey;

pub use super::window_info::{WinDeal, WinVenue, WindowInfo};

/// Что сделать после прохода направления.
#[derive(Debug, Clone, PartialEq)]
pub enum Pass {
    /// Окно открыто сейчас.
    Opened(WindowInfo),
    /// Окно уже было; вошло новое направление — сводка (новые площадки — без tap).
    Joined(WindowInfo),
    /// Окно было, направление уже проходило — только память.
    Same,
}

/// Открытое окно.
#[derive(Debug, Clone)]
struct Open {
    info: WindowInfo,
    passing: HashSet<Direction>,
    /// С какого момента не проходит ни одно направление.
    quiet_since: Option<DateTime<Utc>>,
    /// Решение 241: последняя записанная точка цены DEX по направлению (window_dex.rs).
    dex: super::window_dex::Marks,
}

/// Открытые окна по монетам.
#[derive(Debug, Default)]
pub struct Windows {
    open: HashMap<CoinKey, Open>,
}

impl Windows {
    /// Что делает: направление прошло расчёт. Принимает: направление, id нового окна, сухой ли, прибыль на бюджете,
    /// биржи направления (их пишем), время. Возвращает: Pass. Ошибок нет.
    pub fn pass(
        &mut self,
        d: &Direction,
        id: String,
        dry: bool,
        profit: Decimal,
        cex: &[VenueId],
        now: DateTime<Utc>,
    ) -> Pass {
        let key = CoinKey::of(&d.asset);
        let venue = |v: &VenueId| WinVenue {
            venue: v.clone(),
            asset: d.asset.clone(),
            since: now,
            tap: None,
        };
        if let Some(o) = self.open.get_mut(&key) {
            o.info.max_profit = o.info.max_profit.max(profit);
            o.quiet_since = None;
            if !o.passing.insert(d.clone()) || o.info.directions.contains(d) {
                return Pass::Same;
            }
            o.info.directions.push(d.clone());
            for v in cex {
                if !o.info.venues.iter().any(|w| &w.venue == v) {
                    o.info.venues.push(venue(v));
                }
            }
            return Pass::Joined(o.info.clone());
        }
        let info = WindowInfo {
            id,
            asset: d.asset.clone(),
            dry,
            directions: vec![d.clone()],
            venues: cex.iter().map(venue).collect(),
            started_at: now,
            ended_at: None,
            max_profit: profit,
            max_profit_unbounded: None,
            deals: vec![],
            outcome: None,
            frames: 0,
            lost: 0,
            bytes: 0,
            dex_points: 0,
            maxwins: vec![],
            stopped: None,
            close_reason: None,
        };
        let passing = HashSet::from([d.clone()]);
        let o = Open {
            info: info.clone(),
            passing,
            quiet_since: None,
            dex: Default::default(),
        };
        self.open.insert(key, o);
        Pass::Opened(info)
    }

    /// Что делает: направление перестало проходить. Принимает: направление, время. Возвращает: проходило ли.
    pub fn fail(&mut self, d: &Direction, now: DateTime<Utc>) -> bool {
        let Some(o) = self.open.get_mut(&CoinKey::of(&d.asset)) else {
            return false;
        };
        let was = o.passing.remove(d);
        if was && o.passing.is_empty() {
            o.quiet_since = Some(now);
        }
        was
    }

    /// Проходит ли направление сейчас (в открытом окне). Ошибок нет.
    pub fn passing(&self, d: &Direction) -> bool {
        self.open
            .get(&CoinKey::of(&d.asset))
            .is_some_and(|o| o.passing.contains(d))
    }

    /// Наибольшая прибыль без бюджета из карточки. Принимает: направление, прибыль. Ошибок нет.
    pub fn note_unbounded(&mut self, d: &Direction, v: Decimal) {
        if let Some(o) = self.open.get_mut(&CoinKey::of(&d.asset)) {
            let m = &mut o.info.max_profit_unbounded;
            *m = Some(m.map_or(v, |x| x.max(v)));
        }
    }

    /// Что делает: окна, которые пора закрыть: ни одно направление не проходит дольше hold_ms и сделки по монете
    /// нет (busy). Принимает: время, hold_ms, «идёт ли сделка по монете». Возвращает: ключи. Ошибок нет.
    pub fn closable(
        &self,
        now: DateTime<Utc>,
        hold_ms: u64,
        busy: impl Fn(&CoinKey) -> bool,
    ) -> Vec<CoinKey> {
        self.open
            .iter()
            .filter(|(k, o)| {
                o.quiet_since
                    .is_some_and(|q| (now - q).num_milliseconds() >= hold_ms as i64)
                    && !busy(k)
            })
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Ближайший срок закрытия (тихое окно + hold_ms); None — тихих окон нет. Ошибок нет.
    pub fn deadline(&self, hold_ms: u64) -> Option<DateTime<Utc>> {
        self.open
            .values()
            .filter_map(|o| o.quiet_since)
            .min()
            .map(|q| q + chrono::Duration::milliseconds(hold_ms as i64))
    }

    /// Что делает: закрывает окно. Принимает: ключ, причину, время. Возвращает: сводку; None — не открыто.
    pub fn close(&mut self, k: &CoinKey, reason: &str, now: DateTime<Utc>) -> Option<WindowInfo> {
        let mut info = self.open.remove(k)?.info;
        info.ended_at = Some(now);
        info.close_reason = Some(reason.to_string());
        Some(info)
    }

    /// Решение 241: сводка и памятка точек цены DEX окна, если окно монеты открыто и направление в нём (входило за
    /// время окна); None — нет. Ошибок нет.
    pub fn dex_slot(
        &mut self,
        d: &Direction,
    ) -> Option<(&mut WindowInfo, &mut super::window_dex::Marks)> {
        let o = self.open.get_mut(&CoinKey::of(&d.asset))?;
        o.info
            .directions
            .contains(d)
            .then_some((&mut o.info, &mut o.dex))
    }

    /// Сводка открытого окна монеты (память). Ошибок нет.
    pub fn get(&self, k: &CoinKey) -> Option<WindowInfo> {
        self.open.get(k).map(|o| o.info.clone())
    }

    /// Открытые окна (экран и уборка). Ошибок нет.
    pub fn list(&self) -> Vec<WindowInfo> {
        self.open.values().map(|o| o.info.clone()).collect()
    }

    /// Направления всех открытых окон, которые сейчас проходят. Ошибок нет.
    pub fn all_passing(&self) -> Vec<Direction> {
        self.open
            .values()
            .flat_map(|o| o.passing.iter().cloned())
            .collect()
    }
}
