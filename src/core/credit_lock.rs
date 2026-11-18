//! Файл: core/credit_lock.rs
//! Что делает: решение 413 (t198) — замок счёта (биржа, монета) между ПРОДАЖАМИ сделок и ЧТЕНИЕМ ФАКТА баланса для
//! распределения долей (кейс 4: продажа одной сделки идёт, а в это время зачисляется депозит другой). Продажи берут
//! общий доступ (несколько сделок продают одновременно, как раньше), чтение факта — исключительный: пока ордер продажи
//! в полёте, баланс уже меньше, а учёт продающей сделки ещё нет — такое чтение отдало бы её монеты не той сделке.
//! После продажи, до снятия замка, учёт продающей сделки в памяти обновляется (sync) — следующее чтение факта видит
//! согласованные учёт и баланс. Запись в базу — обычным ledger::save позже (сделка её не ждёт).
//! Ревью t198: чтение факта ждёт продажу не дольше exec.rate_limit_wait_max_ms (fact).
//! Замки — только в памяти (после перезапуска продаж в полёте нет). Пар (биржа, монета) — столько, сколько торговалось.
//! Кто вызывает: core/sell.rs и core/sell_range_run.rs (selling), core/remain_fact.rs и core/credit_base.rs (fact),
//! core/credit_apply.rs (sync).
//! От чего зависит: coin_key.rs, engine.rs (учёт сделок), op_error.rs, tokio::sync::RwLock.
//! Настройки: exec.rate_limit_wait_max_ms (через fact; не задана — ждём, сколько нужно).

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

/// Что делает: исключительный доступ на время чтения факта и снятия учёта других. Ревью t198: продажа держит общий
/// доступ и во время пауз правил шлюза (HTTP 429) — ждём не дольше exec.rate_limit_wait_max_ms (тот же предел, сколько
/// продажа ждёт паузы площадки, решение 201); не дождались — None: читающий считает счёт «продажа в полёте»
/// (Others.busy: доля не выше ожидаемой, замера нет). Настройка не задана — ждём, сколько нужно. Принимает: ядро,
/// площадку, монету. Возвращает: охрану или None. Ошибок нет.
pub async fn fact(core: &Core, v: &VenueId, a: &Asset) -> Option<OwnedRwLockWriteGuard<()>> {
    let lock = of(v, a);
    let max = core
        .inner
        .deps
        .settings
        .integer(
            "exec.rate_limit_wait_max_ms",
            crate::settings::read::At::default(),
        )
        .ok();
    match max {
        Some(ms) => {
            let wait = std::time::Duration::from_millis(ms);
            tokio::time::timeout(wait, lock.write_owned()).await.ok()
        }
        None => Some(lock.write_owned().await),
    }
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
