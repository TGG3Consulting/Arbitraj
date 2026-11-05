//! Файл: core/withdraw_limit.rs
//! Что делает: решение 180 (Тигран, 05.11.2026) — лимит выводов по бирже: не больше transfer.withdraw_max_in_window
//! выводов за transfer.withdraw_window_ms (скользящее окно: в любые последние W мс — не больше N; так лимит биржи не
//! превышается, как бы она ни считала — скользящим или фиксированным окном). Счётчик — в памяти (быстро, без базы).
//! Ключ — биржа или биржа и монета (transfer.withdraw_limit_scope: venue / coin; у Bybit — «once every 10 seconds per
//! chain/coin combination»).
//! - Сделка: место занимается В МОМЕНТ РЕШЕНИЯ (до предпроверки и покупки, executor.rs) — иначе два сигнала подряд
//!   оба пройдут и упрутся в лимит; окно занято — сигнал пропускается («лимит выводов …, свободно через Y мс»).
//!   Место сделки ждёт её вывода (transfers.rs take); сделка закончилась без вывода — место освобождается
//!   (ledger.rs finish → release).
//! - Вывод без места сделки (возврат USDT, перегон, ручной перевод): пропустить нельзя — ждёт свободного места.
//! - Вывод сделан — время места = время вывода (окно считается от вывода, не от решения); повтор того же вывода
//!   (решения 166, 179) обновляет время, лишнего места не занимает.
//!
//! Настройки у биржи не заданы (любая из трёх) — для неё всё ТОЧНО как раньше: ни ожидания, ни пропуска.
//! Ручной вывод с сайта биржи бот не видит — страховка: отказ «слишком часто» → повтор (transfer_locked.rs).
//! Кто вызывает: executor.rs (reserve, bind), transfers.rs (take), ledger.rs (release).
//! От чего зависит: engine.rs, settings.
//! Настройки: transfer.withdraw_window_ms, transfer.withdraw_max_in_window, transfer.withdraw_limit_scope [площадка].

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::core::types::VenueId;
use crate::settings::read::At;

use super::engine::Core;

/// Лимит биржи из настроек.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub window: Duration,
    pub max: u64,
    /// true — ключ «биржа и монета», false — «биржа».
    pub per_coin: bool,
}

/// Окна по ключам: время каждого места (id, когда).
type Windows = Arc<Mutex<HashMap<String, VecDeque<(u64, Instant)>>>>;

/// Окна по ключам (время каждого места) и места сделок, ждущие вывода.
#[derive(Default)]
pub struct State {
    windows: Windows,
    deals: Mutex<HashMap<String, Slot>>,
    next: Mutex<u64>,
}

/// Занятое место. Не использовано (выпало без вывода) — освобождается при удалении (Drop).
pub struct Slot {
    windows: Windows,
    key: String,
    id: u64,
    used: bool,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if !self.used
            && let Some(q) = lock(&self.windows).get_mut(&self.key)
        {
            q.retain(|(i, _)| *i != self.id);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Лимит биржи. None — хоть одна из трёх настроек не задана (как раньше). Ошибок нет.
pub fn limit(core: &Core, venue: &VenueId) -> Option<Limit> {
    let s = &core.inner.deps.settings;
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    let window = s.integer("transfer.withdraw_window_ms", at).ok()?;
    let max = s.integer("transfer.withdraw_max_in_window", at).ok()?;
    let per_coin = match s.text("transfer.withdraw_limit_scope", at).ok()?.as_str() {
        "coin" => true,
        "venue" => false,
        _ => return None,
    };
    (window > 0 && max > 0).then_some(Limit {
        window: Duration::from_millis(window),
        max,
        per_coin,
    })
}

fn key(venue: &VenueId, coin: &str, l: Limit) -> String {
    if l.per_coin {
        format!("{}|{}", venue.0, coin.to_uppercase())
    } else {
        venue.0.clone()
    }
}

/// Чистое решение по окну (без часов — для тестов): выбрасывает места старше окна; свободно — None, занято —
/// через сколько освободится самое старое. Принимает: окно, лимит, «сейчас». Ошибок нет.
pub fn busy(q: &mut VecDeque<(u64, Instant)>, l: Limit, now: Instant) -> Option<Duration> {
    q.retain(|(_, t)| now.saturating_duration_since(*t) < l.window);
    if (q.len() as u64) < l.max {
        return None;
    }
    let oldest = q.iter().map(|(_, t)| *t).min()?;
    Some(
        l.window
            .saturating_sub(now.saturating_duration_since(oldest)),
    )
}

/// Занять место, если свободно. Принимает: ядро, биржу, монету. Возвращает: Ok(None) — лимита нет (как раньше),
/// Ok(Some(место)), Err(текст) — окно занято. Ошибок сверх этого нет.
pub fn reserve(core: &Core, venue: &VenueId, coin: &str) -> Result<Option<Slot>, String> {
    let Some(l) = limit(core, venue) else {
        return Ok(None);
    };
    let st = &core.inner.withdraw_limit;
    let k = key(venue, coin, l);
    let now = Instant::now();
    let mut w = lock(&st.windows);
    let q = w.entry(k.clone()).or_default();
    if let Some(wait) = busy(q, l, now) {
        return Err(format!(
            "лимит выводов {}: {} за {} мс (transfer.withdraw_*) — свободно через {} мс",
            k,
            l.max,
            l.window.as_millis(),
            wait.as_millis()
        ));
    }
    let id = {
        let mut n = lock(&st.next);
        *n += 1;
        *n
    };
    q.push_back((id, now));
    Ok(Some(Slot {
        windows: st.windows.clone(),
        key: k,
        id,
        used: false,
    }))
}

/// Место сделки — ждать её вывода. Принимает: ядро, id сделки, место. Ошибок нет.
pub fn bind(core: &Core, deal: &str, slot: Slot) {
    lock(&core.inner.withdraw_limit.deals).insert(deal.to_string(), slot);
}

/// Сделка закончилась: неиспользованное место освобождается (Drop). Ошибок нет.
pub fn release(core: &Core, deal: &str) {
    lock(&core.inner.withdraw_limit.deals).remove(deal);
}

/// Перед каждым запросом вывода: место сделки или (вывод без места) ожидание свободного; время места — «сейчас».
/// Принимает: ядро, id сделки (пусто — не сделка), биржу, монету. Ошибок нет (лимита нет — сразу).
pub async fn take(core: &Core, deal: &str, venue: &VenueId, coin: &str) {
    let Some(l) = limit(core, venue) else {
        return;
    };
    let st = &core.inner.withdraw_limit;
    let k = key(venue, coin, l);
    let mine = lock(&st.deals)
        .get(deal)
        .filter(|s| s.key == k)
        .map(|s| s.id);
    let id = match mine {
        Some(id) => id,
        None => loop {
            match reserve(core, venue, coin) {
                Ok(Some(mut s)) => {
                    s.used = true;
                    break s.id;
                }
                Ok(None) => return,
                Err(_) => {
                    let wait = {
                        let mut w = lock(&st.windows);
                        let q = w.entry(k.clone()).or_default();
                        busy(q, l, Instant::now()).unwrap_or_default()
                    };
                    tokio::time::sleep(wait.max(Duration::from_millis(1))).await;
                }
            }
        },
    };
    // Замки по одному (порядок «места сделок → окна» — у release через Drop): без взаимной блокировки.
    if let Some(s) = lock(&st.deals).get_mut(deal)
        && s.id == id
    {
        s.used = true;
    }
    let now = Instant::now();
    let mut w = lock(&st.windows);
    let q = w.entry(k).or_default();
    match q.iter_mut().find(|(i, _)| *i == id) {
        Some(e) => e.1 = now,
        // Место выпало из окна, пока сделка покупала, — вывод всё равно идёт (страховка — повтор вывода).
        None => q.push_back((id, now)),
    }
}
