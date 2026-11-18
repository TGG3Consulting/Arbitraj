//! Файл: core/credit_lock.rs
//! Что делает: решение 413 (t198) — замок счёта (биржа, монета) между ПРОДАЖАМИ сделок и ЧТЕНИЕМ ФАКТА баланса для
//! распределения долей (кейс 4: продажа одной сделки идёт, а в это время зачисляется депозит другой). Продажи берут
//! общий доступ (несколько сделок продают одновременно, как раньше), чтение факта — исключительный: пока ордер продажи
//! в полёте, баланс уже меньше, а учёт продающей сделки ещё нет — такое чтение отдало бы её монеты не той сделке.
//! После продажи, до снятия замка, учёт продающей сделки в памяти обновляется (sync) — следующее чтение факта видит
//! согласованные учёт и баланс. Запись в базу — обычным ledger::save позже (сделка её не ждёт).
//! Третье ревью t198 (С-А, С-Ж): чтение факта перед продажей и выводом ждёт конца продаж на счёте (fact); снимок
//! посторонних не ждёт (fact_try: занято — снимка нет).
//! Четвёртое ревью t198 (С-7): охрана продажи держится только на время ордера (trade_seen) и снимается с его итогом,
//! в том числе «исход неизвестен» — выяснение исхода (минуты) идёт без замка. Но учёт такой сделки уже не согласован с
//! балансом: счёт помечается «неясно» (unsure) — распределение долей и снимок посторонних на нём неоднозначны (доля не
//! выше записи × (1 − ожидаемое удержание), замера нет), пока продажа этой сделки не пройдёт, её факт не будет прочитан
//! (remain_fact.rs settle) или она не закроется / не уйдёт к человеку «Беру на себя» (ledger.rs).
//! Замки — только в памяти (после перезапуска продаж в полёте нет). Пар (биржа, монета) — столько, сколько торговалось.
//! Кто вызывает: core/sell.rs и core/sell_range_run.rs (selling), core/remain_fact.rs и core/credit_base.rs (fact),
//! core/credit_apply.rs (sync).
//! От чего зависит: coin_key.rs, engine.rs (учёт сделок), op_error.rs, tokio::sync::RwLock.
//! Настройки: нет (базовое поведение).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::core::types::{Asset, TradeResult, VenueId};

use super::coin_key::CoinKey;
use super::deal::Deal;
use super::engine::{Core, lk};
use super::op_error::OpError;

/// Замки по (площадка, ключ монеты).
type Locks = HashMap<(VenueId, CoinKey), Arc<RwLock<()>>>;

static LOCKS: LazyLock<Mutex<Locks>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Счета «неясно» (С-7): (площадка, ключ монеты) → сделки с исходом продажи «неизвестен».
type Unsure = HashMap<(VenueId, CoinKey), HashSet<String>>;

static UNSURE: LazyLock<Mutex<Unsure>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Что делает: счёт «неясно» — есть сделка с неизвестным исходом продажи (С-7). Ошибок нет.
pub fn unsure(v: &VenueId, a: &Asset) -> bool {
    UNSURE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(v.clone(), CoinKey::of(a)))
        .is_some_and(|s| !s.is_empty())
}

/// Что делает: исход сделки больше не неизвестен (продажа прошла, факт прочитан, закрыта, взята человеком) — снять её
/// отметку со всех счетов. Принимает: ID сделки. Ошибок нет.
pub fn settle(id: &str) {
    let mut m = UNSURE.lock().unwrap_or_else(|p| p.into_inner());
    for s in m.values_mut() {
        s.remove(id);
    }
    m.retain(|_, s| !s.is_empty());
}

/// Замок счёта. Ошибок нет.
fn of(v: &VenueId, a: &Asset) -> Arc<RwLock<()>> {
    LOCKS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entry((v.clone(), CoinKey::of(a)))
        .or_default()
        .clone()
}

/// Что делает: общий доступ на время продажи. Принимает: площадку, монету. Возвращает: охрану (снимается при drop).
/// Ошибок нет.
pub async fn sale(v: &VenueId, a: &Asset) -> OwnedRwLockReadGuard<()> {
    of(v, a).read_owned().await
}

/// Что делает: исключительный доступ на время чтения факта и снятия учёта других — ждём, пока продажи на счёте
/// кончатся (третье ревью t198, С-А: продажа и вывод без факта недопустимы; продажа держит замок только на время своего
/// ордера). Принимает: площадку, монету. Возвращает: охрану. Ошибок нет.
pub async fn fact(v: &VenueId, a: &Asset) -> OwnedRwLockWriteGuard<()> {
    of(v, a).write_owned().await
}

/// Что делает: исключительный доступ без ожидания — для снимка посторонних (третье ревью, С-Ж: снимок замок не ждёт).
/// Принимает: площадку, монету. Возвращает: охрану или None — счёт занят продажей. Ошибок нет.
pub fn fact_try(v: &VenueId, a: &Asset) -> Option<OwnedRwLockWriteGuard<()>> {
    of(v, a).try_write_owned().ok()
}

/// Что делает: учёт сделки в памяти (количество, место, зачисление) — сразу, без базы, чтобы чтение факта другой
/// сделкой видело его (шапка). Принимает: ядро, сделку. Ошибок нет: сделки нет в памяти — ничего.
pub fn sync(core: &Core, deal: &Deal) {
    if let Some(d) = lk(&core.inner.deals).get_mut(&deal.id) {
        d.qty = deal.qty;
        d.at = deal.at.clone();
        d.credit = deal.credit.clone();
    }
}

/// Что делает: продажа сделки под общим доступом к счёту; исполнилось — учёт в памяти сразу уменьшается на проданное
/// (до снятия замка, шапка). Принимает: ядро, сделку (количество до продажи), будущее продажи. Возвращает: итог продажи.
/// Ошибки: как у продажи.
pub async fn selling<F>(core: &Core, deal: &Deal, f: F) -> Result<TradeResult, OpError>
where
    F: Future<Output = Result<TradeResult, OpError>>,
{
    let g = sale(&deal.at, &deal.direction.asset).await;
    let r = f.await;
    if let Ok(x) = &r
        && let Some(d) = lk(&core.inner.deals).get_mut(&deal.id)
    {
        d.qty = deal.qty - x.filled_qty;
    }
    match &r {
        Ok(_) => settle(&deal.id),
        // С-7: исход неизвестен — счёт «неясно», охрана снимается сейчас же (выяснение исхода — без замка).
        Err(e) if e.outcome_unknown() => {
            UNSURE
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .entry((deal.at.clone(), CoinKey::of(&deal.direction.asset)))
                .or_default()
                .insert(deal.id.clone());
        }
        Err(_) => {}
    }
    drop(g);
    r
}
