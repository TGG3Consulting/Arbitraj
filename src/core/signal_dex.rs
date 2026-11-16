//! Файл: core/signal_dex.rs
//! Что делает: решение 352 — котировка стороны DEX сигнала не пересчитывается, пока её входы не менялись. Пересчёт
//! монеты по кадру стакана биржи (в том числе одинаковому — MEXC шлёт снимки потоком, локальные книги отдают верх после
//! любого изменения глубже) идёт как раньше: котировка биржи — заново (дёшево, ставка комиссии биржи — из памяти
//! адаптера, событием ядру не приходит), сигнал засчитывает обновление, удержание и постановка кандидата — те же. Дорогая
//! котировка DEX берётся из памяти, если тот же объём и тот же отпечаток входов:
//! - эпоха адаптера DEX (контракт signal_epoch: голова и цена газа этой головы, минимум каналов, версия реестра пулов,
//!   число расчётов глубины, поколение итогов проверок — налоги и факты пулов, граф, промежуточные, настройки);
//! - поколение пулов монеты в ядре (quote_cache generation: события пулов монеты, снятие пулов, при
//!   signal.mid_pool_wake — события промежуточных пулов) и поколение настроек ядра.
//!
//! Эпохи нет (голова не свежая, лента отстаёт, цены газа этой головы в памяти нет) — котировка заново, как раньше.
//! Почему итог тот же: котировка DEX — функция объёма и входов, перечисленных в docs/notes/t160-signal-sieve-2026-11-16.md
//! (раздел 2.2); всё, что меняет цену и исполнимый объём, — в отпечатке; газ разрешений и чаевые (от кошелька) в сигнал
//! не входят (prices.rs fee_usdt — только TradeFee/PoolFee), поэтому котировка общая для кошельков всех бирж. Значит
//! цены, спред, порог и состояние сигнала — те же, что при котировке заново, и кандидат появляется в тот же пересчёт.
//! Память — последний отпечаток на (площадка, токен, сторона); новый отпечаток стирает прежние объёмы.
//! Включается необязательной signal.book_same_skip; не задана или нет — 1 в 1 как раньше (prices.rs side_quote).
//! Кто вызывает: core/prices.rs (side_quote), core/quote_count_log.rs (counts).
//! От чего зависит: engine.rs, dex_account.rs, venue_ticker.rs, quote_cache.rs (generation), gateway (signal_epoch,
//! quote), settings (generation).
//! Настройки: signal.book_same_skip.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Direction, Quote, Side, VenueId};
use crate::settings::read::At;

use super::engine::{Core, lk};

/// Ключ: площадка DEX, токен, покупка (true) / продажа.
type Key = (VenueId, Address, bool);

/// Отпечаток входов котировки: эпоха адаптера, поколение пулов монеты в ядре, поколение настроек.
type Stamp = (String, u64, u64);

/// Память: ключ → (отпечаток, объём → котировка).
#[derive(Default)]
pub struct Memo(Mutex<HashMap<Key, (Stamp, HashMap<Decimal, Quote>)>>);

/// Котировок из памяти и заново (при включённой настройке).
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// Счётчики с запуска: (из памяти, заново). Ошибок нет.
pub fn counts() -> (u64, u64) {
    (HITS.load(Ordering::Relaxed), MISSES.load(Ordering::Relaxed))
}

/// Включено ли (signal.book_same_skip). Ошибок нет.
pub fn on(core: &Core) -> bool {
    let s = &core.inner.deps.settings;
    s.flag("signal.book_same_skip", At::default())
        .unwrap_or(false)
}

/// Что делает: котировка стороны DEX сигнала — из памяти при том же отпечатке и объёме, иначе заново (и в память).
/// Принимает: ядро, направление, площадку DEX, адрес токена, сторону, объём.
/// Возвращает: котировку. Ошибки: текст — как у prices.rs side_quote (нет кошелька в маппинге, ошибка котировки).
pub async fn quote(
    core: &Core,
    dir: &Direction,
    venue: &VenueId,
    token: Address,
    side: Side,
    qty: Decimal,
) -> Result<Quote, String> {
    let key = (venue.clone(), token, side == Side::Buy);
    // Отпечаток — ДО котировки: состояние, сменившееся во время котировки, даст другой отпечаток следующему вызову
    // (номер головы, версии и поколения только растут).
    let now = stamp(core, venue, token).await;
    if let Some(st) = &now
        && let Some(q) = get(core, &key, st, qty)
    {
        HITS.fetch_add(1, Ordering::Relaxed);
        return Ok(q);
    }
    MISSES.fetch_add(1, Ordering::Relaxed);
    let deps = &core.inner.deps;
    // Решение 132: DEX — кошелёк строки маппинга направления; нет строки — «нет кошелька DEX в маппинге».
    let a = super::dex_account::venue_dir(deps, dir, venue).await?;
    // Решение 211: площадке — её тикер монеты; ответ — обратно в монету ядра.
    let on = super::venue_ticker::on_venue(core, &dir.asset, venue);
    let q = a
        .quote(&on, side, qty)
        .await
        .map_err(|e| format!("{}: {}", venue.0, e.message))?;
    let q = super::venue_ticker::quote_back(core, &dir.asset, venue, q);
    if let Some(st) = now {
        put(core, key, st, qty, q.clone());
    }
    Ok(q)
}

/// Отпечаток сейчас. None — у площадки нет эпохи (биржа, голова не свежая и т. п.) или площадка не подключена.
/// Ошибок нет.
async fn stamp(core: &Core, venue: &VenueId, token: Address) -> Option<Stamp> {
    let epoch = core.inner.deps.gw.get(venue).await.ok()?.signal_epoch()?;
    let pools = lk(&core.inner.quote_cache).generation(venue, token);
    Some((epoch, pools, core.inner.deps.settings.generation()))
}

/// Котировка из памяти: тот же отпечаток и объём. Ошибок нет.
fn get(core: &Core, key: &Key, stamp: &Stamp, qty: Decimal) -> Option<Quote> {
    let m = lk(&core.inner.signal_dex.0);
    let (st, by_qty) = m.get(key)?;
    if st != stamp {
        return None;
    }
    by_qty.get(&qty).cloned()
}

/// Запоминает котировку; другой отпечаток стирает прежние объёмы ключа. Ошибок нет.
fn put(core: &Core, key: Key, stamp: Stamp, qty: Decimal, q: Quote) {
    let mut m = lk(&core.inner.signal_dex.0);
    let slot = m
        .entry(key)
        .or_insert_with(|| (stamp.clone(), HashMap::new()));
    if slot.0 != stamp {
        *slot = (stamp, HashMap::new());
    }
    slot.1.insert(qty, q);
}
