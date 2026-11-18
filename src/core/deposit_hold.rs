//! Файл: core/deposit_hold.rs
//! Что делает: решение 413 (t198) — УДЕРЖАНИЕ БИРЖИ ПРИ ЗАЧИСЛЕНИИ как мера по паре (биржа, монета). Живой случай
//! 18.11 (WKC, налог токена 2 % на перевод, DEX → MEXC): MEXC записал депозиты ровно на 98 % отправленного (налог перевода
//! учтён верно), а на спот зачислил ровно 98 % записей (сверка 21:38:19: учёт 6 036 003 243.34, на MEXC
//! 5 915 283 179.1468). Механизм биржа не описывает (внутренний перевод с адреса депозита — догадка, не факт) — бот
//! опирается только на факт баланса.
//!
//! Замер (факт): последнее однозначное распределение факта счёта между ждущими долю сделками (credit_split.rs,
//! measurable) — записи R, досталось сделкам C, удержано % = (R − C) / R × 100 (не меньше 0). Хранится сырым в учёте ядра
//! (SQLite, вид deposit_hold), как факты переводов (withdraw_seen.rs): последний замер заменяет прежний; 0 % — тоже факт
//! (биржа не удерживает) и снимает консервативное допущение.
//!
//! Ожидаемое удержание (expected_pct) — в расчёт прибыли и количества СЛЕДУЮЩИХ сделок (строка затрат DepositHold
//! «перевод», в монете: приход на B меньше) и в распределение:
//! - получатель DEX или нативная монета — 0 (кошелёк получает по событию Transfer = зачислено, решение 151/222);
//! - замер есть — он;
//! - замера нет — налог перевода токена (transfer_tax::measured, проверки налогов DEX): у WKC удержание равно ему —
//!   консервативно, чтобы первая сделка не ушла в минус так, как WKC; налога нет (0) — 0 (монеты без налога токена —
//!   поведение прежнее); налог не измерен — 0 (такие сделки и так не проходят расчёт: transfer_tax.rs).
//!
//! suspect — монета, перед продажей которой на бирже нужен свежий факт баланса (sell_fact.rs): ожидаемое удержание > 0
//! или налог токена не измерен.
//! Кто вызывает: core/costs_withdraw.rs (строка затрат), core/relay_value.rs (оценка «Перегона»), core/credit_apply.rs
//! (ожидание и запись замера), core/sell_fact.rs (suspect), core/coins_view.rs (экран «Монеты»), core/engine.rs (load).
//! От чего зависит: persist.rs, transfer_tax.rs, native.rs, trade_ops.rs (вид площадки), engine.rs, core/types.
//! Настройки: нет (мера — данные учёта, не настройка).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{Asset, Cost, CostKind, VenueId};
use crate::gateway::contract::VenueKind;
use crate::storage::journal::Level;

use super::coin_key::CoinKey;
use super::engine::{Core, rd};

/// Вид записи в учёте ядра.
const KIND: &str = "deposit_hold";

/// Замер удержания по (бирже, монете).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hold {
    pub venue: VenueId,
    pub asset: Asset,
    /// Сумма записей депозитов ждущих сделок в замере.
    pub records: Decimal,
    /// Досталось сделкам по факту баланса.
    pub credited: Decimal,
    /// Удержано, % от записей.
    pub pct: Decimal,
    pub at: DateTime<Utc>,
    /// Сделки замера.
    pub deals: Vec<String>,
}

/// Замеры: (биржа, ключ монеты) → замер. Одно ядро на процесс.
static HOLDS: LazyLock<Mutex<HashMap<(VenueId, CoinKey), Hold>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// id записи в учёте: площадка и адрес (или тикер). Ошибок нет.
fn id(v: &VenueId, a: &Asset) -> String {
    let coin = a
        .bsc_contract
        .map(|x| x.to_string())
        .unwrap_or(a.ticker.clone());
    format!("{}:{coin}", v.0)
}

/// Что делает: загрузка замеров при запуске. Ошибка базы — журнал (замеры начнутся с новых зачислений). Ошибок нет.
pub async fn load(core: &Core) {
    match super::persist::load::<Hold>(&core.inner.deps.db, KIND).await {
        Ok(list) => {
            let mut m = HOLDS.lock().unwrap_or_else(|p| p.into_inner());
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

/// Что делает: записывает замер (заменяет прежний) и сохраняет. Ошибок нет: сбой базы — журнал.
pub async fn record(core: &Core, h: Hold) {
    let rid = id(&h.venue, &h.asset);
    put(h.clone());
    if let Err(e) = super::persist::save(&core.inner.deps.db, KIND, &rid, &h).await {
        let m = format!("Замер удержания {rid} не сохранён: {e}");
        core.inner
            .deps
            .log(Level::Error, "core.credit_fact", None, m, None)
            .await;
    }
}

/// Что делает: кладёт замер в память (без базы; тесты и record). Ошибок нет.
pub fn put(h: Hold) {
    HOLDS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert((h.venue.clone(), CoinKey::of(&h.asset)), h);
}

/// Замер по бирже и монете. None — не измерено. Ошибок нет.
pub fn get(v: &VenueId, a: &Asset) -> Option<Hold> {
    HOLDS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(v.clone(), CoinKey::of(a)))
        .cloned()
}

/// Все замеры монеты (экран «Монеты»). Ошибок нет.
pub fn of_coin(a: &Asset) -> Vec<Hold> {
    let key = CoinKey::of(a);
    let mut out: Vec<Hold> = HOLDS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|((_, k), _)| *k == key)
        .map(|(_, h)| h.clone())
        .collect();
    out.sort_by(|x, y| x.venue.0.cmp(&y.venue.0));
    out
}

/// Что делает: ожидаемое удержание по чистым данным (правило — шапка). Принимает: получатель — биржа, монета нативная,
/// замер (%), налог перевода токена (%; None — не измерен). Возвращает: %. Ошибок нет.
pub fn expected_of(
    cex: bool,
    native: bool,
    measured: Option<Decimal>,
    token_tax: Option<Decimal>,
) -> Decimal {
    if !cex || native {
        return Decimal::ZERO;
    }
    measured
        .or(token_tax)
        .unwrap_or_default()
        .max(Decimal::ZERO)
}

/// Ожидаемое удержание биржи `to` при зачислении монеты, %. Ошибок нет.
pub fn expected_pct(core: &Core, to: &VenueId, a: &Asset) -> Decimal {
    let cex = super::trade_ops::kind(core, to) == Ok(VenueKind::Cex);
    let native = super::native::is_native(core, a);
    expected_of(cex, native, get(to, a).map(|h| h.pct), token_tax(core, a))
}

/// Нужен ли свежий факт баланса перед продажей монеты на бирже `v` (шапка). Ошибок нет.
pub fn suspect(core: &Core, v: &VenueId, a: &Asset) -> bool {
    if super::native::is_native(core, a) {
        return false;
    }
    expected_pct(core, v, a) > Decimal::ZERO
        || (get(v, a).is_none() && token_tax(core, a).is_none())
}

/// Налог перевода токена по проверкам DEX, %. None — не измерен или нет адреса. Ошибок нет.
fn token_tax(core: &Core, a: &Asset) -> Option<Decimal> {
    let t = a.bsc_contract?;
    let kinds = rd(&core.inner.kinds).clone();
    super::transfer_tax::measured(&rd(&core.inner.market), &kinds, t)
}

/// Что делает: строка затрат «удержание биржи при зачислении» на приход. Принимает: процент, приход в монете (вывод
/// минус затраты вывода в монете), тикер. Возвращает: затрата DepositHold в монете (None — 0 %). Ошибок нет.
pub fn line(pct: Decimal, arrive: Decimal, ticker: &str) -> Option<Cost> {
    (pct > Decimal::ZERO && arrive > Decimal::ZERO).then(|| {
        Cost::new(
            CostKind::DepositHold,
            arrive * pct / Decimal::ONE_HUNDRED,
            ticker.to_string(),
            false,
        )
    })
}

/// Что делает: добавляет к затратам вывода строку удержания получателя `to` (приход — сумма минус затраты в монете).
/// Принимает: ядро, получателя, монету, сумму вывода, затраты вывода. Возвращает: затраты. Ошибок нет.
pub fn add(core: &Core, to: &VenueId, a: &Asset, amount: Decimal, mut c: Vec<Cost>) -> Vec<Cost> {
    let in_coin: Decimal = c
        .iter()
        .filter(|x| !x.in_price && super::convert::in_coin(x, &a.ticker))
        .map(|x| x.amount)
        .sum();
    if let Some(l) = line(expected_pct(core, to, a), amount - in_coin, &a.ticker) {
        c.push(l);
    }
    c
}
