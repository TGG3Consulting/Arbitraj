//! Файл: core/signal_dex.rs
//! Что делает: решение 352 — котировка стороны DEX сигнала не пересчитывается, пока её входы не менялись. Пересчёт
//! монеты по кадру стакана биржи (в том числе одинаковому — MEXC шлёт снимки потоком, локальные книги отдают верх после
//! любого изменения глубже) идёт как раньше: котировка биржи — заново (дёшево, ставка комиссии биржи — из памяти
//! адаптера, событием ядру не приходит), сигнал засчитывает обновление, удержание и постановка кандидата — те же. Дорогая
//! котировка DEX берётся из памяти, если тот же объём и тот же отпечаток входов:
//! - эпоха адаптера DEX для монеты (контракт signal_epoch: голова и цена газа этой головы, минимум каналов, версия
//!   реестра пулов, число расчётов глубины, поколение итогов проверок — налоги и факты пулов, граф, промежуточные,
//!   настройки, наборы путей монеты в кэше с их устареванием);
//! - поколение пулов монеты в ядре (quote_cache generation: события пулов монеты, снятие пулов, события промежуточных
//!   пулов — mid_wake.rs) и поколение настроек ядра.
//!
//! Эпохи нет (голова не свежая, лента отстаёт, цены газа этой головы в памяти нет) — котировка заново, как раньше.
//! Почему итог тот же: котировка DEX — функция объёма и входов, перечисленных в docs/notes/t160-signal-sieve-2026-11-16.md
//! (раздел 2.2); всё, что меняет цену и исполнимый объём, — в отпечатке; газ разрешений и чаевые (от кошелька) в сигнал
//! не входят (prices.rs fee_usdt — только TradeFee/PoolFee), поэтому котировка общая для кошельков всех бирж. Значит
//! цены, спред, порог и состояние сигнала — те же, что при котировке заново, и кандидат появляется в тот же пересчёт.
//! Память — последний отпечаток на (площадка, токен, сторона); новый отпечаток стирает прежние объёмы.
//! Включается необязательной signal.book_same_skip; не задана или нет — 1 в 1 как раньше (prices.rs side_quote).
//! Под настройкой (Тигран 16.11: что тестами не доказано — настройкой): основа проверена тестом
//! tests/bsc_dex_signal_epoch_352.rs (повторная котировка на том же состоянии побитово та же, эпоха меняется от каждого
//! входа), но полной котировки DEX через ядро (голова, цена газа с узла, кошелёк маппинга) на записи нет — равенство
//! «из памяти = заново» и моменты кандидата сквозь ядро тестом не проверены.
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

/// Ключ: площадка DEX, токен, покупка (true) / продажа, кошелёк строки маппинга направления (ревью t160 Б4).
type Key = (VenueId, Address, bool, Option<String>);

/// Отпечаток входов котировки: эпоха адаптера, поколение пулов монеты в ядре, поколение настроек.
type Stamp = (String, u64, u64);

/// Запись памяти одного ключа: отпечаток и котировки по объёмам.
type Slot = (Stamp, HashMap<Decimal, Quote>);

/// Память: ключ → (отпечаток, объём → котировка).
#[derive(Default)]
pub struct Memo(Mutex<HashMap<Key, Slot>>);

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
/// Ревью t160: сначала кошелёк строки маппинга направления (решение 132; нет строки — та же ошибка, что раньше, до
/// чтения памяти — Б4); у площадки нет эпохи (агрегатор KyberSwap, голова не свежая) — None: вызывающий идёт прежним
/// путём с signal.dex_quote_cache_ms (Б5).
/// Принимает: ядро, направление, площадку DEX, адрес токена, сторону, объём.
/// Возвращает: Some(котировка или текст ошибки — как у prices.rs side_quote); None — эпохи нет. Ошибок нет.
pub async fn quote(
    core: &Core,
    dir: &Direction,
    venue: &VenueId,
    token: Address,
    side: Side,
    qty: Decimal,
) -> Option<Result<Quote, String>> {
    let deps = &core.inner.deps;
    let acc = match super::dex_account::of_direction(deps, dir).await {
        Ok(acc) => acc,
        Err(e) => return Some(Err(e)),
    };
    let a = match deps.gw.get_as(venue, acc.as_ref()).await {
        Ok(a) => a,
        Err(e) => return Some(Err(e.message)),
    };
    // Отпечаток — ДО котировки: состояние, сменившееся во время котировки, даст другой отпечаток следующему вызову.
    let epoch = a.signal_epoch(&dir.asset)?;
    let pools = lk(&core.inner.quote_cache).generation(venue, token);
    let st: Stamp = (epoch, pools, deps.settings.generation());
    let key = (venue.clone(), token, side == Side::Buy, acc.map(|x| x.0));
    if let Some(q) = get(core, &key, &st, qty) {
        HITS.fetch_add(1, Ordering::Relaxed);
        return Some(Ok(q));
    }
    MISSES.fetch_add(1, Ordering::Relaxed);
    // Решение 211: площадке — её тикер монеты; ответ — обратно в монету ядра.
    let on = super::venue_ticker::on_venue(core, &dir.asset, venue);
    let q = match a.quote(&on, side, qty).await {
        Ok(q) => q,
        Err(e) => return Some(Err(format!("{}: {}", venue.0, e.message))),
    };
    let q = super::venue_ticker::quote_back(core, &dir.asset, venue, q);
    put(core, key, st, qty, q.clone());
    Some(Ok(q))
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
