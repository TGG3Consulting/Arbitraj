//! Файл: venues/bsc_dex/pools/link.rs
//! Что делает: состояние связи слушателей пулов с узлом BSC по WebSocket — одно на площадку, а не на пачку подписки.
//! Обрыв у сотен пачек (сбой сети) — одна запись «связь потеряна» (первая причина) и одна «восстановлена» (сколько
//! подписок прерывалось), а не по записи на каждую пачку и попытку.
//!
//! Решение 339 (журнал обрывов, t153): кроме этого, КАЖДЫЙ обрыв сессии пачки — своя запись venue.ws (Warn): имя
//! пачки, число пулов, причина целиком. Одинаковая запись (та же пачка, та же причина без чисел и hex) — не чаще раза
//! в log.pool_break_interval_s секунд, пропущенные считаются и называются в следующей записи; не задана — каждый
//! обрыв. Без этого причина повторных обрывов не видна (запись «потеряна» — одна, пока не восстановятся все пачки).
//! Кто вызывает: pools/listen.rs (down — сессия кончилась), pools/session.rs (up — подписка и блок получены).
//! От чего зависит: pools/listen.rs (Pools), pools/progress.rs (Signals хранит состояние).
//! Настройки: log.pool_break_interval_s [вся система] (необязательная).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::storage::journal::Level;

use super::listen::Pools;

/// Состояние связи.
#[derive(Default)]
pub struct State {
    /// Пачки без связи.
    down: HashSet<String>,
    /// Сколько пачек прерывалось за этот обрыв.
    lost: HashSet<String>,
    /// Запись «потеряна» уже сделана.
    told: bool,
    /// Записи об обрыве пачки по ключу (пачка, вид причины): когда была последняя и сколько похожих пропущено с неё.
    seen: HashMap<(String, String), (Instant, u64)>,
}

/// Настройка: одинаковая запись об обрыве пачки — не чаще раза в столько секунд.
pub const INTERVAL: &str = "log.pool_break_interval_s";

/// Вид причины для ограничения частоты: числа убраны, слова с «0x» (хеши, адреса, данные отката) — «0x…».
/// Принимает: причину. Возвращает: вид. Ошибок нет.
pub fn shape(why: &str) -> String {
    why.split_whitespace()
        .map(|w| {
            if w.contains("0x") {
                "0x…".to_string()
            } else {
                w.chars().filter(|c| !c.is_ascii_digit()).collect()
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Что делает: писать ли запись об обрыве по ключу сейчас и сколько похожих пропущено с прошлой. Первая запись
/// ключа — всегда; дальше — если интервал не задан или прошёл. Принимает: состояние, ключ, интервал, «сейчас».
/// Возвращает: (писать, пропущено). Ошибок нет.
pub fn due(
    s: &mut State,
    key: (String, String),
    every: Option<Duration>,
    now: Instant,
) -> (bool, u64) {
    match s.seen.get_mut(&key) {
        Some((at, skipped)) => {
            if every.is_none_or(|ev| now.saturating_duration_since(*at) >= ev) {
                let n = *skipped;
                *at = now;
                *skipped = 0;
                (true, n)
            } else {
                *skipped += 1;
                (false, 0)
            }
        }
        None => {
            s.seen.insert(key, (now, 0));
            (true, 0)
        }
    }
}

/// Сессия пачки кончилась. Первая — запись «связь потеряна»; каждая — запись об обрыве пачки (см. шапку).
/// Принимает: окружение, имя пачки, причину, были ли данные (тогда они недостоверны), число пулов пачки. Ошибок нет.
pub async fn down(p: &Pools, name: &str, why: &str, loaded: bool, pools: usize) {
    let every = p
        .ctx
        .int(INTERVAL)
        .ok()
        .filter(|&n| n > 0)
        .map(Duration::from_secs);
    let (first, (write, skipped)) = {
        let mut s = p.signals.link.lock().unwrap_or_else(|x| x.into_inner());
        s.down.insert(name.to_string());
        s.lost.insert(name.to_string());
        let first = !std::mem::replace(&mut s.told, true);
        let key = (name.to_string(), shape(why));
        (first, due(&mut s, key, every, Instant::now()))
    };
    if write {
        let more = if skipped > 0 {
            format!(
                "; таких же обрывов этой пачки без записи с прошлой записи: {skipped} ({INTERVAL})"
            )
        } else {
            String::new()
        };
        let msg = format!(
            "Пачка пулов {name} ({pools} пулов): сессия подписки прервана — {why}; переподключение через \
             venue.ws_reconnect_pause_ms{more}"
        );
        p.ctx.log(Level::Warn, "venue.ws", msg, None).await;
    }
    if first {
        let tail = if loaded {
            "данные пулов недостоверны до восстановления"
        } else {
            "подписка или первая загрузка пулов не удалась, повтор"
        };
        let msg = format!("Связь с узлом BSC (WebSocket) потеряна: {why} — {tail}");
        p.ctx.log(Level::Warn, "venue.ws", msg, None).await;
    }
}

/// Сессия пачки началась (подписка есть). Последняя восстановленная после обрыва — запись «восстановлена». Ошибок нет.
pub async fn up(p: &Pools, name: &str) {
    let lost = {
        let mut s = p.signals.link.lock().unwrap_or_else(|x| x.into_inner());
        s.down.remove(name);
        if s.told && s.down.is_empty() {
            s.told = false;
            Some(std::mem::take(&mut s.lost).len())
        } else {
            None
        }
    };
    if let Some(n) = lost {
        let msg = format!("Связь с узлом BSC (WebSocket) восстановлена: прерывалось подписок {n}");
        p.ctx.log(Level::Info, "venue.ws", msg, None).await;
    }
}
