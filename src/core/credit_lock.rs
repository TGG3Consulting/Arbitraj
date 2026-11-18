//! Файл: core/credit_lock.rs
//! Что делает: решение 413 (t198) — замок счёта (биржа, монета) между ПРОДАЖАМИ сделок и ЧТЕНИЕМ ФАКТА баланса для
//! распределения долей (кейс 4: продажа одной сделки идёт, а в это время зачисляется депозит другой). Продажи берут
//! общий доступ (несколько сделок продают одновременно, как раньше), чтение факта — исключительный: пока ордер продажи
//! в полёте, баланс уже меньше, а учёт продающей сделки ещё нет — такое чтение отдало бы её монеты не той сделке.
//! После продажи, до снятия замка, учёт продающей сделки в памяти обновляется (sync) — следующее чтение факта видит
//! согласованные учёт и баланс. Запись в базу — обычным ledger::save позже (сделка её не ждёт).
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
use super::deal::Deal;
use super::engine::{Core, lk};
use super::op_error::OpError;

/// Замки по (площадка, ключ монеты).
type Locks = HashMap<(VenueId, CoinKey), Arc<RwLock<()>>>;

static LOCKS: LazyLock<Mutex<Locks>> = LazyLock::new(|| Mutex::new(HashMap::new()));

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

/// Что делает: исключительный доступ на время чтения факта и снятия учёта других. Принимает: площадку, монету.
/// Возвращает: охрану. Ошибок нет.
pub async fn fact(v: &VenueId, a: &Asset) -> OwnedRwLockWriteGuard<()> {
    of(v, a).write_owned().await
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
    let _g = sale(&deal.at, &deal.direction.asset).await;
    let r = f.await;
    if let Ok(x) = &r
        && let Some(d) = lk(&core.inner.deals).get_mut(&deal.id)
    {
        d.qty = deal.qty - x.filled_qty;
    }
    r
}
