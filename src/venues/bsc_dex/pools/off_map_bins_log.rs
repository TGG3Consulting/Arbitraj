//! Файл: venues/bsc_dex/pools/off_map_bins_log.rs
//! Что делает: ревью решения 332 (Н2, Н7) — ограничение частоты записей журнала dex.pool о проверке «цена ушла за
//! прочитанную полосу бинов» (pools/off_map_bins.rs): Info «пул перечитывается» и Warn «проверка не считается»
//! (стоимость бина не считается, decimals встречного токена неизвестны) — каждая не чаще раза в
//! log.bin_reread_interval_s секунд на пул (первая — сразу). Не задана — этих записей нет (перечитывание и проверка
//! работают как без неё; как у log.pool_events_lost_interval_s, решение 313). Отметки — в памяти процесса, по пулу и
//! виду записи (их не больше, чем пулов из бинов × 2).
//! Кто вызывает: pools/off_map_bins.rs (after).
//! От чего зависит: ctx.rs (настройка), core/types (PoolRef).
//! Настройки: log.bin_reread_interval_s (вся система; необязательная).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;

/// Имя настройки интервала записей.
pub const INTERVAL: &str = "log.bin_reread_interval_s";

/// Вид записи журнала (у каждого свои часы).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum What {
    /// Info: пул перечитывается вокруг нового активного бина.
    Reread,
    /// Warn: проверка не считается — пул может остаться «мелким» молча (Н2).
    Unknown,
}

/// Когда была прошлая запись по (пул, вид).
static LAST: Mutex<Option<HashMap<(PoolRef, What), Instant>>> = Mutex::new(None);

/// Пора ли писать запись `w` о пуле `r`: интервал задан и с прошлой такой записи прошло не меньше него (первая — сразу).
/// Пора — отметка ставится сразу (вызывающий пишет). Принимает: окружение, пул, вид. Возвращает: true — писать.
/// Ошибок нет (интервал не задан или не читается — false).
pub fn due(ctx: &Ctx, r: &PoolRef, w: What) -> bool {
    let Ok(secs) = ctx.int(INTERVAL) else {
        return false;
    };
    pass(
        &mut LAST.lock().unwrap_or_else(|p| p.into_inner()),
        r,
        w,
        secs,
        Instant::now(),
    )
}

/// Чистая логика due (без настройки и часов процесса). Принимает: отметки, пул, вид, интервал, сейчас. Возвращает: true
/// — пора (отметка обновлена). Ошибок нет.
fn pass(
    last: &mut Option<HashMap<(PoolRef, What), Instant>>,
    r: &PoolRef,
    w: What,
    secs: u64,
    now: Instant,
) -> bool {
    let m = last.get_or_insert_with(HashMap::new);
    let key = (r.clone(), w);
    if let Some(t) = m.get(&key)
        && now.saturating_duration_since(*t) < Duration::from_secs(secs)
    {
        return false;
    }
    m.insert(key, now);
    true
}
