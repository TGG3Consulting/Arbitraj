//! Файл: venues/bsc_dex/pools/link.rs
//! Что делает: состояние связи слушателей пулов с узлом BSC по WebSocket — одно на площадку, а не на пачку подписки.
//! Обрыв у сотен пачек (сбой сети) — одна запись «связь потеряна» (первая причина) и одна «восстановлена» (сколько
//! подписок прерывалось), а не по записи на каждую пачку и попытку.
//!
//! Кто вызывает: pools/listen.rs (down — сессия кончилась), pools/session.rs (up — подписка и блок получены).
//! От чего зависит: pools/listen.rs (Pools), pools/progress.rs (Signals хранит состояние).
//! Настроек нет.

use std::collections::HashSet;

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
}

/// Сессия пачки кончилась. Первая — запись «связь потеряна». Принимает: окружение, имя пачки, причину,
/// были ли данные (тогда они недостоверны). Ошибок нет.
pub async fn down(p: &Pools, name: &str, why: &str, loaded: bool) {
    let first = {
        let mut s = p.signals.link.lock().unwrap_or_else(|x| x.into_inner());
        s.down.insert(name.to_string());
        s.lost.insert(name.to_string());
        !std::mem::replace(&mut s.told, true)
    };
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
