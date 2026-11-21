//! Файл: core/deposit_hold.rs
//! Что делает: решение 413 (t198) — УДЕРЖАНИЕ БИРЖИ ПРИ ЗАЧИСЛЕНИИ как мера по паре (биржа, монета): хранение замеров.
//! Живой случай 18.11 (WKC, налог токена 2 % на перевод, DEX → MEXC): MEXC записал депозиты ровно на 98 % отправленного,
//! а на спот зачислил ровно 98 % записей (сверка 21:38:19: учёт 6 036 003 243.34, на MEXC 5 915 283 179.1468).
//! Механизм биржа не описывает — бот опирается только на факт баланса.
//!
//! Замер (выборка) — однозначное распределение факта счёта (credit_apply.rs measurable: одна ждущая, снимок
//! посторонних, никого в пути, без лишнего, доля не пыль, не ручное действие): записи R, досталось C, % = (R − C) / R.
//! Ревью t198 (С2): хранятся последние N выборок (N — transfer.deposit_hold_samples, по бирже; не задана — 1: последняя),
//! мера — НАИБОЛЬШИЙ процент из них. Почему максимум, а не медиана: мера идёт в затраты следующих сделок — недооценка
//! удержания ведёт сделку в минус (живой WKC), переоценка только отсекает часть сделок; одна аномальная выборка держит
//! меру не дольше N следующих зачислений, и её видно на экране («Сбросить замер» — снять). 0 % — тоже факт.
//! Учёт ядра SQLite (вид deposit_hold), «Сбросить замер» — reset (журнал core.credit_fact с автором, как изменения
//! настроек). Расчёт (ожидаемое удержание, строка затрат) — deposit_hold_calc.rs.
//! Кто вызывает: core/credit_apply.rs (put, save), core/deposit_hold_calc.rs (get), core/coins_view.rs (of_coin),
//! api/deposit_hold.rs (reset), core/engine.rs (load).
//! От чего зависит: persist.rs, coin_key.rs, engine.rs, settings, core/types.
//! Настройки: transfer.deposit_hold_samples.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{Asset, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::coin_key::CoinKey;
use super::engine::Core;

pub use super::deposit_hold_calc::{add, expected_of, expected_pct, line};

/// Вид записи в учёте ядра.
const KIND: &str = "deposit_hold";

/// Одна выборка замера.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// Запись депозита ждущей сделки.
    pub records: Decimal,
    /// Досталось сделке по факту баланса.
    pub credited: Decimal,
    /// Удержано, % от записи.
    pub pct: Decimal,
    pub at: DateTime<Utc>,
    pub deals: Vec<String>,
}

/// Замер по (бирже, монете): последние выборки и мера (наибольший % из них).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hold {
    pub venue: VenueId,
    pub asset: Asset,
    pub samples: Vec<Sample>,
    /// Мера: наибольший % выборок.
    pub pct: Decimal,
}

static HOLDS: LazyLock<Mutex<HashMap<(VenueId, CoinKey), Hold>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Замеры. Ошибок нет.
fn holds() -> std::sync::MutexGuard<'static, HashMap<(VenueId, CoinKey), Hold>> {
    HOLDS.lock().unwrap_or_else(|p| p.into_inner())
}

/// id записи в учёте: площадка и адрес (или тикер); решение 487 (сети в id) — у монеты другой сети «…@сеть»
/// (coin_key::rid; ключ памяти — CoinKey с сетью), сеть по умолчанию — id прежний. Ошибок нет.
fn id(v: &VenueId, a: &Asset) -> String {
    format!("{}:{}", v.0, super::coin_key::rid(a))
}

/// Что делает: загрузка замеров при запуске. Ошибка базы — журнал. Ошибок нет.
pub async fn load(core: &Core) {
    match super::persist::load::<Hold>(&core.inner.deps.db, KIND).await {
        Ok(list) => {
            let mut m = holds();
            for h in list {
                m.insert((h.venue.clone(), CoinKey::of(&h.asset)), h);
            }
        }
        Err(e) => {
            let m = format!("Замеры удержания при зачислении (решение 413) не загружены: {e}");
            core.inner
                .deps
                .log(Level::Error, "core.credit_fact", None, m, None)
                .await;
        }
    }
}

/// Что делает: мера по выборкам (чистая функция): наибольший % последних `n` (n < 1 — 1). Принимает: выборки (старые
/// первыми), n. Возвращает: (оставшиеся выборки, мера). Ошибок нет.
pub fn fold(mut samples: Vec<Sample>, n: usize) -> (Vec<Sample>, Decimal) {
    let n = n.max(1);
    if samples.len() > n {
        samples.drain(..samples.len() - n);
    }
    let pct = samples
        .iter()
        .map(|s| s.pct)
        .fold(Decimal::ZERO, Decimal::max);
    (samples, pct)
}

/// Сколько выборок держать (transfer.deposit_hold_samples по бирже; не задана — 1). Ошибок нет.
fn keep_n(core: &Core, v: &VenueId) -> usize {
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    core.inner
        .deps
        .settings
        .integer("transfer.deposit_hold_samples", at)
        .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
        .unwrap_or(1)
}

/// Что делает: добавляет выборку в память (мера пересчитывается сразу — расчёт следующих сделок видит её). Ошибок нет.
pub fn put(core: &Core, v: &VenueId, a: &Asset, s: Sample) {
    let n = keep_n(core, v);
    let mut m = holds();
    let key = (v.clone(), CoinKey::of(a));
    let mut list = m.get(&key).map(|h| h.samples.clone()).unwrap_or_default();
    list.push(s);
    let (samples, pct) = fold(list, n);
    m.insert(
        key,
        Hold {
            venue: v.clone(),
            asset: a.clone(),
            samples,
            pct,
        },
    );
}

/// Что делает: сохраняет замер (биржа, монета) в учёт ядра. Ошибок нет: сбой базы — журнал.
pub async fn save(core: &Core, v: &VenueId, a: &Asset) {
    let Some(h) = get(v, a) else { return };
    let rid = id(v, a);
    if let Err(e) = super::persist::save(&core.inner.deps.db, KIND, &rid, &h).await {
        let m = format!("Замер удержания {rid} не сохранён: {e}");
        core.inner
            .deps
            .log(Level::Error, "core.credit_fact", None, m, None)
            .await;
    }
}

/// Замер по бирже и монете. None — не измерено. Ошибок нет.
pub fn get(v: &VenueId, a: &Asset) -> Option<Hold> {
    holds().get(&(v.clone(), CoinKey::of(a))).cloned()
}

/// Все замеры монеты (экран «Монеты»). Ошибок нет.
pub fn of_coin(a: &Asset) -> Vec<Hold> {
    let key = CoinKey::of(a);
    let mut out: Vec<Hold> = holds()
        .iter()
        .filter(|((_, k), _)| *k == key)
        .map(|(_, h)| h.clone())
        .collect();
    out.sort_by(|x, y| x.venue.0.cmp(&y.venue.0));
    out
}

/// Что делает: «Сбросить замер» (ревью t198, С2): замер (биржа, монета) удаляется из памяти и учёта; дальше —
/// «не измерено» (в расчёте — налог перевода токена). Журнал core.credit_fact: кто, что было. Принимает: ядро, биржу,
/// монету, автора. Возвращает: true — замер был. Ошибки: текст — сбой базы.
pub async fn reset(core: &Core, v: &VenueId, a: &Asset, who: &str) -> Result<bool, String> {
    let was = holds().remove(&(v.clone(), CoinKey::of(a)));
    let Some(h) = was else { return Ok(false) };
    super::persist::remove(&core.inner.deps.db, KIND, &id(v, a))
        .await
        .map_err(|e| e.to_string())?;
    let text = format!(
        "Замер удержания при зачислении {} на {} сброшен ({who}): было {}% по {} выборкам",
        a.ticker,
        v.0,
        h.pct,
        h.samples.len()
    );
    core.inner
        .deps
        .log(Level::Info, "core.credit_fact", Some(&v.0), text, None)
        .await;
    Ok(true)
}
