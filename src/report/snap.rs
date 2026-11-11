//! Файл: report/snap.rs
//! Что делает: решение 257 (Тигран 11.11 п. 10; доработка по приёмке t90) — данные листа «Слепок по балансам» отчёта
//! «Итоги сделок» в момент выгрузки:
//! - биржи реестра — все счета (Venue::account_balances, чтение адаптера; память панели «Балансы» не трогается), цена —
//!   report/snap_price.rs (котировка площадки, только чтение — торговлю не меняет);
//! - кошельки DEX — report/snap_dex.rs; монеты в пути и площадки вне реестра — report/snap_transit.rs;
//! - сделки с деньгами у площадки (открытые и остановленные / «Беру на себя» с монетами) — строкой там, где сейчас их
//!   монеты или деньги по шагу сделки: покупка — площадка покупки; перевод — «В пути»; продажа, застряла, ждёт
//!   человека, остановлена — площадка Deal.at (у DEX — кошелёк сделки).
//!
//! Общий срок — настройка report.snapshot_timeout_ms (не задана — слепок не снимается, причина на листе, отчёт
//! выдаётся). Не ответила площадка — «нет данных: причина», не 0.
//! Одна функция для кнопки интерфейса и Telegram (report/results.rs results_file).
//! Кто вызывает: report/results.rs; report/cap_send.rs (решение 256 — тот же слепок в «Сделках, лимитах и
//! капитале»).
//! От чего зависит: core (deals, cex_ids, настройки), core/deal_text.rs, report/snap_cex.rs, snap_dex.rs, snap_transit.rs,
//! notify/human.rs.
//! Настройки: report.snapshot_timeout_ms; venue.quote_asset (читается).

use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::time::Instant;

use crate::core::Core;
use crate::core::deal::{Deal, Step};
use crate::core::types::VenueId;
use crate::notify::human::{money, qty as q, venue as name};
use crate::settings::read::At;

/// Настройка общего срока слепка.
pub const TIMEOUT: &str = "report.snapshot_timeout_ms";

/// Строка: счёт, монета, количество (Err — нет данных), цена за монету в USDT (Err — нет цены), пометка, под вопросом.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub account: String,
    pub asset: String,
    pub qty: Result<f64, String>,
    pub price: Result<f64, String>,
    pub note: String,
    /// Под вопросом: по фактам сделки не определить, ушёл ли вывод — монета, возможно, ещё и в балансе площадки;
    /// в итог не входит, отдельной суммой «под вопросом» (snap_transit.rs).
    pub doubt: bool,
}

/// Площадка (биржа, кошелёк DEX, «В пути»): имя, время ответа, нет данных вовсе (причина), неполно (причины),
/// строки, сделки (текст), что не учитывается (причины).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Place {
    pub name: String,
    pub at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub partial: Vec<String>,
    pub lines: Vec<Line>,
    pub deals: Vec<String>,
    pub unknown: Vec<String>,
}

/// Слепок: площадки; problem — слепок не снят (причина).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    pub places: Vec<Place>,
    pub problem: Option<String>,
}

/// Где деньги или монеты сделки по шагу.
#[derive(Debug, Clone, PartialEq)]
pub enum Loc {
    Venue(VenueId),
    Transit,
}

/// Сделка держит деньги у площадки: открытая или остановленная / «Беру на себя» с монетами. Ошибок нет.
pub fn holds(d: &Deal) -> bool {
    d.is_open()
        || (matches!(d.step, Step::Halted | Step::TakenOver) && d.qty > rust_decimal::Decimal::ZERO)
}

/// Где сейчас деньги или монеты сделки (см. шапку). Ошибок нет.
pub fn loc(d: &Deal) -> Loc {
    match d.step {
        Step::Buying => Loc::Venue(d.direction.buy_on.clone()),
        Step::Transfer => Loc::Transit,
        _ => Loc::Venue(d.at.clone()),
    }
}

/// Строка сделки. Ошибок нет.
pub fn deal_line(d: &Deal) -> String {
    let what = if d.is_open() {
        "активная сделка"
    } else {
        "сделка с монетами у площадки"
    };
    let place = match loc(d) {
        Loc::Transit => format!("в пути от {} (вывод начат)", name(&d.at.0)),
        Loc::Venue(v) => format!("на {}", name(&v.0)),
    };
    format!(
        "На момент отчёта {what} {}: {} {} → {}, шаг «{}», размером {} USDT (монеты: {} {} {place})",
        d.id,
        d.direction.asset.ticker,
        name(&d.direction.buy_on.0),
        name(&d.direction.sell_on.0),
        crate::core::deal_text::step_name(d.step),
        money(d.invested),
        q(d.qty),
        d.direction.asset.ticker,
    )
}

/// Слепок (см. шапку). Принимает: ядро. Возвращает: слепок. Ошибок нет: сбои — в площадках, без срока — problem.
pub async fn gather(core: &Core) -> Snapshot {
    let ms = match core.inner.deps.settings.integer(TIMEOUT, At::default()) {
        Ok(v) => v,
        Err(e) => {
            return Snapshot {
                places: vec![],
                problem: Some(format!("слепок не снят: {}", e.message)),
            };
        }
    };
    let deadline = Instant::now() + Duration::from_millis(ms);
    let all = core.deals();
    let held: Vec<&Deal> = all.iter().filter(|d| holds(d)).collect();
    let mut used = vec![false; held.len()];
    let mut out = Vec::new();
    let mut cex = core.cex_ids();
    cex.sort_by(|a, b| a.0.cmp(&b.0));
    for v in &cex {
        let mut p = super::snap_cex::place(core, v, deadline).await;
        for (k, d) in held.iter().enumerate() {
            if loc(d) == Loc::Venue(v.clone()) {
                p.deals.push(deal_line(d));
                used[k] = true;
            }
        }
        out.push(p);
    }
    out.extend(super::snap_dex::wallets(core, &held, &mut used, deadline).await);
    out.push(super::snap_transit::transit(core, &all, &held, &mut used, deadline).await);
    out.extend(super::snap_transit::orphans(&held, &used));
    Snapshot {
        places: out,
        problem: None,
    }
}
