//! Файл: core/ride_trail.rs
//! Что делает: решение 394 (t179) — «след» продаж сделок групп докупки. Пауза направления после удачной сделки группы
//! снята (route_pause::won), а продажа идущей сделки группы уже уменьшила её монеты впереди (ride_quote::sell_ahead
//! берёт остаток) — значит, новый расчёт продажи той же монеты на той же площадке не должен идти по стакану / пулу,
//! которые нашу продажу ещё не отражают. След: время конца последней продажи (биржа), её блок и пулы (DEX). Расчёт
//! продажи (costs_source.rs Live) проверяет: у биржи — стакан получен позже конца продажи; у DEX — маршрут через её
//! пулы (пулы одного контракта — один пул, PoolRef::site) по блоку не раньше её блока; маршрут или блок неизвестны —
//! «данные не доказуемо свежее» (отказ, расчёт повторит следующий тик). Проверка прошла по стакану / её пулам — след
//! снят. Пишется только у сделок с группой докупки (exec.top_up = да): выключено — следа нет, ничего не меняется.
//! Решение 463 (t238): след ставится и сразу при учёте продажи в памяти (credit_lock::selling → note_fill), блок следа —
//! самый высокий из продаж ключа.
//! Кто вызывает: ledger.rs (save → note), credit_lock.rs (note_fill), ride_quote.rs (check).
//! От чего зависит: coin_key.rs, deal.rs, trade_ops.rs (вид площадки), engine.rs, core/types.
//! Настройки: нет.

use std::sync::atomic::Ordering;

use chrono::{DateTime, Utc};

use crate::core::types::{PoolRef, Quote, TradeResult, VenueId};
use crate::gateway::contract::VenueKind;

use super::coin_key::CoinKey;
use super::deal::Deal;
use super::engine::{Core, lk};

/// След последней продажи монеты сделкой группы на площадке.
#[derive(Debug, Clone, PartialEq)]
pub struct Trail {
    /// Сделка.
    pub deal: String,
    /// Конец продажи (ответ площадки).
    pub at: DateTime<Utc>,
    /// DEX: блок транзакции продажи; None — неизвестен (у биржи — всегда None).
    pub block: Option<u64>,
    /// DEX: пулы маршрута продажи (PoolRef::site); пусто — маршрут неизвестен (агрегатор).
    pub pools: Vec<PoolRef>,
}

/// Пулы маршрута (пулы одного контракта — один). Ошибок нет.
pub fn sites(r: &crate::core::types::DexRoute) -> Vec<PoolRef> {
    let mut v: Vec<PoolRef> = r
        .legs
        .iter()
        .flat_map(|l| l.hops.iter().map(|h| h.pool.site()))
        .collect();
    v.dedup();
    v
}

/// Что делает: сделка группы сохранена — последняя её продажа (если новее записанной) становится следом монеты на
/// площадке продажи. Принимает: ядро, сделку. Ошибок нет; сделка без группы или без продаж — ничего.
pub fn note(core: &Core, deal: &Deal) {
    if deal.group.is_none() {
        return;
    }
    let Some(s) = deal.sells.iter().max_by_key(|s| s.finished_at) else {
        return;
    };
    put(core, deal, s);
}

/// Что делает: решение 463 (t238) — след продажи сделки группы СРАЗУ, когда её учёт в памяти уменьшен на проданное
/// (credit_lock::selling), а не только при сохранении (ledger::save идёт позже): между ними расчёт новой сделки видел
/// бы остаток без проданного, а пул — возможно, ещё без продажи (пропуск её монет). Принимает: ядро, сделку (до
/// продажи), итог продажи. Ошибок нет; без группы или без исполнения — ничего.
pub fn note_fill(core: &Core, deal: &Deal, r: &TradeResult) {
    if deal.group.is_none() || r.filled_qty <= rust_decimal::Decimal::ZERO {
        return;
    }
    put(core, deal, r);
}

/// След из продажи `s` сделки: решение 463 — у одного ключа хранится самый поздний конец и САМЫЙ ВЫСОКИЙ блок из
/// продаж (продажи разных сделок могут кончиться не в порядке блоков), пулы — объединение. Ошибок нет.
fn put(core: &Core, deal: &Deal, s: &TradeResult) {
    let mut t = Trail {
        deal: deal.id.clone(),
        at: s.finished_at,
        block: s.chain.as_ref().and_then(|c| c.block),
        pools: s.request.route.as_ref().map(sites).unwrap_or_default(),
    };
    let key = (CoinKey::of(&deal.direction.asset), s.request.venue.clone());
    let mut m = lk(&core.inner.trail);
    match m.get(&key) {
        // Блок неизвестен хоть у одной продажи — у следа «неизвестен» (свежесть не доказуема, как раньше).
        Some(old) if old.at >= t.at && (old.block.is_none() || old.block >= t.block) => {}
        Some(old) => {
            if old.at > t.at {
                t.at = old.at;
                t.deal = old.deal.clone();
            }
            t.block = match (t.block, old.block) {
                (Some(a), Some(b)) => Some(a.max(b)),
                _ => None,
            };
            // Маршрут одной из продаж неизвестен — пулы следа «неизвестны» (проверка по любому маршруту).
            if old.pools.is_empty() || t.pools.is_empty() {
                t.pools.clear();
            } else {
                for p in &old.pools {
                    if !t.pools.contains(p) {
                        t.pools.push(p.clone());
                    }
                }
            }
            m.insert(key, t);
        }
        None => {
            m.insert(key, t);
            core.inner.group_live.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Что делает: живая котировка продажи `q` на площадке `v` новее следа продажи сделки группы (см. шапку); прошла по
/// стакану или по пулам следа — след снят. Принимает: ядро, монету, площадку продажи, котировку.
/// Ошибки: текст — данные не новее продажи (или не доказуемо новее).
pub fn check(core: &Core, coin: &CoinKey, v: &VenueId, q: &Quote) -> Result<(), String> {
    let key = (coin.clone(), v.clone());
    let Some(t) = lk(&core.inner.trail).get(&key).cloned() else {
        return Ok(());
    };
    let kind = super::trade_ops::kind(core, v)?;
    let fresh = match (kind, &q.route) {
        (VenueKind::Cex, _) => q.at > t.at,
        (VenueKind::Dex, Some(r)) => {
            let mine = sites(r);
            let touches = t.pools.is_empty() || mine.iter().any(|p| t.pools.contains(p));
            if !touches {
                return Ok(());
            }
            t.block.is_some_and(|b| r.block >= b)
        }
        (VenueKind::Dex, None) => false,
    };
    if !fresh {
        return Err(format!(
            "{}: данные продажи не доказуемо свежее продажи сделки {} ({}{}) — ждём данные после неё",
            v.0,
            t.deal,
            t.at,
            t.block.map(|b| format!(", блок {b}")).unwrap_or_default()
        ));
    }
    let mut m = lk(&core.inner.trail);
    if m.get(&key).is_some_and(|x| x.at == t.at) && m.remove(&key).is_some() {
        core.inner.group_live.fetch_sub(1, Ordering::Relaxed);
    }
    Ok(())
}
