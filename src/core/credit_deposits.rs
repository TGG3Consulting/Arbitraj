//! Файл: core/credit_deposits.rs
//! Что делает: решение 413 (четвёртое ревью t198, С-3) — «другие депозиты монеты после снимка». Однозначный замер
//! удержания снимает с доли сделки ограничение запись × (1 − ожидаемое удержание) только если в истории депозитов биржи
//! (Venue::deposit_hashes — тот же официальный эндпоинт, что поиск записи депозита по хешу) нет депозитов этой монеты
//! на этот счёт, кроме депозита самой сделки и тех, что уже были при снимке посторонних (credit_base.rs). Иначе
//! ограничение остаётся (ручной депозит не уходит сделке), остаток — «не присвоено, перечитать» (credit_fact::rest).
//! Список при снимке читается вместе со снимком посторонних (своя задача, вывод не ждёт) и привязывается к хешу вывода
//! вместе с ним; в учёте сделки — CreditFact.deposits.
//! Кто вызывает: core/credit_base.rs (spawn, bind, forget), core/credit_fact.rs (take), core/credit_apply.rs (clean).
//! От чего зависит: gateway (deposit_hashes), gateway/tx_hash.rs, engine.rs.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::core::types::{Asset, VenueId};
use crate::gateway::tx_hash;

use super::deal::Deal;
use super::engine::Core;

/// Хеши депозитов (элемент None — запись без хеша).
pub type Hashes = Vec<Option<String>>;

/// Списки при снимке: номер перевода → список (None — не прочитан).
static READS: LazyLock<Mutex<HashMap<u64, Option<Hashes>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Списки, привязанные к хешу вывода (нижний регистр, без 0x).
static BOUND: LazyLock<Mutex<HashMap<String, Hashes>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Хеши депозитов монеты на бирже. Ошибки — текст.
async fn list(core: &Core, v: &VenueId, a: &Asset) -> Result<Hashes, String> {
    let gw = core.inner.deps.gw.get(v).await.map_err(|e| e.message)?;
    gw.deposit_hashes(a).await.map_err(|e| e.message)
}

/// Что делает: читает список депозитов при снимке перевода `key` (своей задачей). Принимает: ядро, номер, биржу,
/// монету. Ошибок нет: не прочитан — None (ограничение доли останется).
pub fn spawn(core: &Core, key: u64, to: &VenueId, a: &Asset) {
    READS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, None);
    let (c, to, a) = (core.clone(), to.clone(), a.clone());
    tokio::spawn(async move {
        let got = list(&c, &to, &a).await.ok();
        let mut m = READS.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(slot) = m.get_mut(&key) {
            *slot = got;
        }
    });
}

/// Что делает: привязывает список перевода `key` к хешу вывода (вместе со снимком посторонних). Ошибок нет.
pub fn bind(key: u64, norm: String) {
    let got = READS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&key)
        .flatten();
    if let Some(l) = got {
        BOUND
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(norm, l);
    }
}

/// Что делает: перевод сорван — список забывается. Ошибок нет.
pub fn forget(key: u64) {
    READS.lock().unwrap_or_else(|p| p.into_inner()).remove(&key);
}

/// Что делает: забирает список по хешу вывода (нормализованному). Ошибок нет.
pub fn take(norm: &str) -> Option<Hashes> {
    BOUND.lock().unwrap_or_else(|p| p.into_inner()).remove(norm)
}

/// Что делает: новые депозиты (чистая функция) — хеши `now`, которых нет в `before` и которые не депозит сделки `own`;
/// запись без хеша — всегда «новая» (неизвестно). Принимает: списки и хеш сделки. Возвращает: сколько новых. Ошибок нет.
pub fn new_count(before: &[Option<String>], now: &[Option<String>], own: &str) -> usize {
    now.iter()
        .filter(|h| match h {
            None => true,
            Some(h) => {
                !tx_hash::same(h, own)
                    && !before
                        .iter()
                        .any(|b| b.as_deref().is_some_and(|b| tx_hash::same(b, h)))
            }
        })
        .count()
}

/// Что делает: других депозитов монеты на счёт после снимка нет (С-3). Принимает: ядро, сделку (ждёт доли, запись с
/// хешем и списком при снимке). Возвращает: да — нет (снять ограничение можно); нет — есть, список не снят или не
/// прочитан. Ошибок нет.
pub async fn clean(core: &Core, deal: &Deal) -> bool {
    let Some(c) = super::credit_fact::here(deal) else {
        return false;
    };
    let (Some(own), Some(before)) = (c.tx.clone(), c.deposits.clone()) else {
        return false;
    };
    match list(core, &deal.at, &deal.direction.asset).await {
        Ok(now) => new_count(&before, &now, &own) == 0,
        Err(_) => false,
    }
}
