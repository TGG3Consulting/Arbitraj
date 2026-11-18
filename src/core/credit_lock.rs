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
//! балансом: отметка сделки Deal.sale_unsure (пятое ревью: в учёте SQLite — переживает перезапуск) делает счёт «неясным»
//! (unsure): распределение долей и снимок посторонних на нём неоднозначны (доля не окончательна, замера нет), её монеты
//! — не «подтверждённые» других (remain_others.rs: в пути / неясно), пока продажа этой сделки не пройдёт или её факт не
//! будет прочитан (remain_fact.rs); закрытая / взятая человеком сделка счёт не путает. Человеку — остановка сделки
//! «исход продажи неизвестен» (sell.rs).
//! Замки — только в памяти (после перезапуска продаж в полёте нет). Пар (биржа, монета) — столько, сколько торговалось.
//! Кто вызывает: core/sell.rs и core/sell_range_run.rs (selling), core/remain_fact.rs и core/credit_base.rs (fact),
//! core/credit_apply.rs (sync).
//! От чего зависит: coin_key.rs, engine.rs (учёт сделок), op_error.rs, tokio::sync::RwLock.
//! Настройки: нет (базовое поведение).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::core::types::{Asset, TradeResult, VenueId};

use super::coin_key::CoinKey;
use super::deal::{Deal, Step};
use super::engine::{Core, lk};
use super::op_error::OpError;

/// Замки по (площадка, ключ монеты).
type Locks = HashMap<(VenueId, CoinKey), Arc<RwLock<()>>>;

static LOCKS: LazyLock<Mutex<Locks>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Что делает: счёт «неясно» — на нём открытая сделка той же монеты с неизвестным исходом продажи (Deal.sale_unsure,
/// С-7). Принимает: ядро, площадку, монету. Ошибок нет.
pub fn unsure(core: &Core, v: &VenueId, a: &Asset) -> bool {
    let key = CoinKey::of(a);
    lk(&core.inner.deals).values().any(|d| {
        d.sale_unsure
            && &d.at == v
            && !matches!(d.step, Step::Closed | Step::TakenOver)
            && CoinKey::of(&d.direction.asset) == key
    })
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
    if let Some(d) = lk(&core.inner.deals).get_mut(&deal.id) {
        match &r {
            Ok(x) => {
                d.qty = deal.qty - x.filled_qty;
                d.sale_unsure = false;
            }
            // С-7: исход неизвестен — счёт «неясно» сразу (учёт памяти; запись в базу — вызывающий, sell.rs), охрана
            // снимается сейчас же (выяснение исхода — без замка).
            Err(e) if e.outcome_unknown() => d.sale_unsure = true,
            Err(_) => {}
        }
    }
    drop(g);
    r
}
