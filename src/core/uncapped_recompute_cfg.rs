//! Файл: core/uncapped_recompute_cfg.rs
//! Что делает: решение 218 — части подготовки пересчёта старой записи «без потолка» по окну (uncapped_recompute_ctx.rs):
//! что берётся из старой записи (бой, бюджет, было упущено и потолок), настройки модели (как у uncapped.rs compute),
//! площадка в контексте без данных сети (на момент сделки их в базе нет) и шаг поиска.
//! Шаг поиска — 10^-n, n — наибольшее число знаков у количества плана и количеств уровней стакана A (количества кратны
//! шагу биржи — поиск не грубее него; правил пары на момент сделки в базе нет).
//! Кто вызывает: core/uncapped_recompute_ctx.rs, core/uncapped_recompute.rs, uncapped_recompute_all.rs.
//! От чего зависит: uncapped_types.rs (Battle), limits_parts.rs, size_uncapped.rs (At), registry.rs (Presence),
//! uncapped_record.rs (ladder_in), uncapped_replay.rs (Plan), settings.
//! Настройки: card.max_search_usdt, money.min_profit_usd, money.min_profit_pct (решение 242: площадка + тип направления —
//! core/venue_dir.rs), uncapped.max_evals, uncapped.ladder_usdt.

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::core::types::{Asset, OrderBook, VenueId};
use crate::settings::read::At as SAt;
use crate::settings::store::Store;
use crate::settings::venue_dir::VenueDir;

use super::limits_parts::Parts;
use super::registry::Presence;
use super::size_uncapped::At;
use super::uncapped_replay::Plan;
use super::uncapped_types::Battle;

/// Что нужно из старой записи (Record в JSON).
#[derive(Debug, Clone, Deserialize)]
pub struct Old {
    pub battle: Battle,
    #[serde(default)]
    pub parts: Option<Parts>,
    #[serde(default)]
    pub limits_error: Option<String>,
    #[serde(default)]
    pub budget: Option<Decimal>,
    pub opt: At,
    pub missed: Decimal,
}

impl Old {
    /// Разбор старой записи. Ошибки: текст — запись не читается.
    pub fn parse(body: &str) -> Result<Old, String> {
        serde_json::from_str(body).map_err(|e| format!("старая запись не читается: {e}"))
    }
}

/// Настройки модели (как у uncapped.rs compute).
pub struct Cfg {
    pub search_cap: Decimal,
    pub min_usd: Decimal,
    pub min_pct: Decimal,
    pub max_evals: u32,
    pub ladder: Vec<Decimal>,
}

impl Cfg {
    /// Чтение из настроек. Принимает: хранилище, площадку и тип направления сделки (решение 242; None — нет).
    /// Ошибки: текст — не задана настройка.
    pub fn read(s: &Store, vd: Option<VenueDir>) -> Result<Cfg, String> {
        let dec = |n: &str| s.decimal(n, SAt::default()).map_err(|e| e.message);
        let max_evals = s
            .integer("uncapped.max_evals", SAt::default())
            .map_err(|e| e.message)?;
        let search_cap = dec("card.max_search_usdt")?;
        let (min_usd, min_pct) = super::venue_dir::thresholds(s, vd)?;
        Ok(Cfg {
            search_cap,
            min_usd,
            min_pct,
            max_evals: u32::try_from(max_evals).map_err(|_| "uncapped.max_evals вне пределов")?,
            ladder: super::uncapped_record::ladder_in(s)?,
        })
    }
}

/// Область настройки «площадка». Ошибок нет.
pub fn venue_at(v: &VenueId) -> SAt<'_> {
    SAt {
        venue: Some(&v.0),
        coin: None,
    }
}

/// Площадка в контексте пересчёта: без данных сети (их на момент сделки нет). Ошибок нет.
pub fn presence(v: &VenueId, a: &Asset) -> Presence {
    Presence {
        venue: v.clone(),
        ticker: a.ticker.clone(),
        network: None,
        can_receive: true,
        can_send: true,
        can_buy: true,
        can_sell: true,
        excluded: vec![],
        info: None,
        via: None,
    }
}

/// Шаг поиска: 10^-n, n — наибольшее число знаков у количества плана и уровней стакана A. Ошибок нет.
pub fn step_of(plan: &Plan, a: &OrderBook) -> Decimal {
    let sc = |x: Decimal| x.normalize().scale();
    let n = a
        .asks
        .iter()
        .map(|l| sc(l.qty))
        .fold(sc(plan.qty), u32::max)
        .min(18);
    Decimal::new(1, n)
}
