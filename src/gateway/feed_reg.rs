//! Файл: gateway/feed_reg.rs
//! Что делает: решение 202 — общий для адаптеров учёт свежести потока стаканов (FeedReg): соединение регистрирует свои
//! пары, на каждом кадре отмечает время, получает команды сторожа ядра из своего канала; снимок свежести и доставка
//! команд для методов контракта feed_health / resubscribe_pair / reconnect_conn. Решение 213: на кадре — ещё и версия
//! книги из кадра (номер и/или время изменения по часам биржи), чтобы ядро сравнило её с версией по REST.
//! Кто вызывает: адаптеры бирж (соединения стаканов и методы контракта).
//! От чего зависит: gateway/feed_health.rs (типы), gateway/errors.rs, tokio (канал команд), chrono.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

use tokio::sync::mpsc;

use super::feed_health::{Cmd, ConnHealth, FeedHealth, PairHealth};

/// Сейчас, unix мс.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// «Не было» для версии книги (номер может быть любым числом, поэтому не 0).
const NO_VER: i64 = i64::MIN;

/// Учёт одной пары. Время: 0 — «не было»; версия: NO_VER — «не было».
struct Pair {
    ticker: String,
    /// Последний кадр стакана (часы бота).
    last: AtomicI64,
    /// (Пере)подписка — начало прогрева.
    since: AtomicI64,
    /// Решение 213: номер версии книги из последнего кадра.
    id: AtomicI64,
    /// Решение 213: время изменения книги из последнего кадра (часы биржи).
    changed: AtomicI64,
}

/// Учёт одного соединения: время кадров, пары, канал команд. 0 в атомике — «не было».
struct Conn {
    name: String,
    alive: AtomicI64,
    started: AtomicI64,
    ended: AtomicI64,
    pairs: Vec<Pair>,
    idx: HashMap<String, usize>,
    tx: mpsc::UnboundedSender<Cmd>,
}

/// Учёт свежести соединений адаптера. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct FeedReg {
    conns: Arc<RwLock<Vec<Arc<Conn>>>>,
}

/// Ручка соединения: отметки времени без поиска по всем соединениям. Дешёво клонируется.
#[derive(Clone)]
pub struct ConnHandle {
    conn: Arc<Conn>,
}

impl FeedReg {
    /// Новая подписка адаптера: прежние соединения забыты (их задачи остановлены). Ошибок нет.
    pub fn clear(&self) {
        self.conns
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Регистрирует соединение и его пары (тикеры площадки, решение 211). Возвращает: ручку и канал команд соединения.
    pub fn register(
        &self,
        name: &str,
        tickers: &[String],
    ) -> (ConnHandle, mpsc::UnboundedReceiver<Cmd>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let conn = Arc::new(Conn {
            name: name.to_string(),
            alive: AtomicI64::new(0),
            started: AtomicI64::new(0),
            ended: AtomicI64::new(0),
            idx: tickers
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, t)| (t, i))
                .collect(),
            pairs: tickers
                .iter()
                .map(|t| Pair {
                    ticker: t.clone(),
                    last: AtomicI64::new(0),
                    since: AtomicI64::new(0),
                    id: AtomicI64::new(NO_VER),
                    changed: AtomicI64::new(NO_VER),
                })
                .collect(),
            tx,
        });
        self.conns
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(conn.clone());
        (ConnHandle { conn }, rx)
    }

    /// Причина отсутствия локальной книги на пути отказа котировки. Реестр отражает
    /// назначение пары основной WS-сессии, а не подтверждение подписки биржей.
    pub fn missing_book(&self, ticker: &str) -> String {
        let conns = self.conns.read().unwrap_or_else(|e| e.into_inner());
        let Some((conn, pair)) = conns
            .iter()
            .find_map(|c| c.idx.get(ticker).map(|i| (c, &c.pairs[*i])))
        else {
            return "стакана нет: пары нет в текущем реестре подписок WS".into();
        };
        let ended = conn.ended.load(Ordering::Acquire);
        let state = if ended != 0 {
            format!("сессия не активна с {ended} (Unix мс; причина в venue.ws)")
        } else if conn.started.load(Ordering::Relaxed) == 0 {
            "сессия ещё не запущена".into()
        } else if pair.last.load(Ordering::Relaxed) == 0 {
            "первый кадр пары в текущей сессии ещё не наблюдался".into()
        } else {
            "кадр пары уже наблюдался, пригодная книга в памяти отсутствует".into()
        };
        format!("стакана нет: основной WS {}, {state}", conn.name)
    }

    /// Снимок свежести. Принимает: признаки площадки (по расписанию, переподписка пары, подтверждение по REST —
    /// решение 213). Ошибок нет.
    pub fn health(&self, scheduled: bool, per_pair: bool, confirm: bool) -> FeedHealth {
        self.health_for(scheduled, per_pair, confirm, None)
    }

    /// Для ответа REST копируются только данные его подписки; реестр читается в момент обработки.
    pub fn health_for(
        &self,
        scheduled: bool,
        per_pair: bool,
        confirm: bool,
        pair: Option<(&str, &str)>,
    ) -> FeedHealth {
        let opt = |v: i64| (v > 0).then_some(v);
        let ver = |a: &AtomicI64| Some(a.load(Ordering::Relaxed)).filter(|&v| v != NO_VER);
        let conns = self
            .conns
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|c| pair.is_none_or(|(conn, _)| c.name == conn))
            .map(|c| ConnHealth {
                conn: c.name.clone(),
                alive_ms: opt(c.alive.load(Ordering::Relaxed)),
                started_ms: opt(c.started.load(Ordering::Relaxed)),
                pairs: c
                    .pairs
                    .iter()
                    .filter(|p| pair.is_none_or(|(_, ticker)| p.ticker == ticker))
                    .map(|p| PairHealth {
                        ticker: p.ticker.clone(),
                        last_ms: opt(p.last.load(Ordering::Relaxed)),
                        since_ms: p.since.load(Ordering::Relaxed),
                        last_id: ver(&p.id),
                        last_changed_ms: ver(&p.changed),
                    })
                    .collect(),
            })
            .collect();
        FeedHealth {
            scheduled,
            per_pair,
            confirm,
            conns,
        }
    }

    /// Команда соединению для методов контракта (resubscribe_pair, reconnect_conn). Принимает: площадку, имя
    /// соединения, команду. Ошибки: BadResponse — соединения нет или оно остановлено (текст причины).
    pub fn command(
        &self,
        venue: crate::core::types::VenueId,
        conn: &str,
        cmd: Cmd,
    ) -> super::errors::GatewayResult<()> {
        self.send(conn, cmd).map_err(|m| {
            super::errors::GatewayError::new(super::errors::ErrorCode::BadResponse, Some(venue), m)
        })
    }

    /// Команда соединению по имени. Ошибка — текст: соединения нет или его задача уже закончилась.
    pub fn send(&self, conn: &str, cmd: Cmd) -> Result<(), String> {
        let list = self.conns.read().unwrap_or_else(|e| e.into_inner());
        let c = list
            .iter()
            .find(|c| c.name == conn)
            .ok_or_else(|| format!("соединения {conn} нет в текущей подписке"))?;
        c.tx.send(cmd)
            .map_err(|_| format!("соединение {conn} уже остановлено"))
    }
}

impl ConnHandle {
    /// Новая сессия (подключились): прогрев соединения и всех пар — с этого момента; прежние кадры и их версии
    /// забыты. Ошибок нет.
    pub fn session_started(&self) {
        let now = now_ms();
        self.conn.started.store(now, Ordering::Relaxed);
        self.conn.alive.store(now, Ordering::Relaxed);
        for p in &self.conn.pairs {
            p.last.store(0, Ordering::Relaxed);
            p.since.store(now, Ordering::Relaxed);
            p.id.store(NO_VER, Ordering::Relaxed);
            p.changed.store(NO_VER, Ordering::Relaxed);
        }
        self.conn.ended.store(0, Ordering::Release);
    }

    /// Попытка подключения завершилась; текст сбоя уже пишет цикл соединения.
    pub fn session_ended(&self) {
        self.conn.ended.store(now_ms(), Ordering::Release);
    }

    /// Кадр соединения любого вида. Ошибок нет.
    pub fn alive(&self) {
        self.conn.alive.store(now_ms(), Ordering::Relaxed);
    }

    /// Кадр стакана пары (и кадр соединения), без версии книги. Неизвестный тикер — только кадр соединения.
    /// Ошибок нет.
    pub fn frame(&self, ticker: &str) {
        self.frame_ver(ticker, None, None);
    }

    /// Решение 213: кадр стакана пары с версией книги из кадра — номер (та же шкала, что BookVersion.id по
    /// документации биржи) и/или время изменения по часам биржи (та же шкала, что BookVersion.changed_ms). Отмечать на
    /// КАЖДОМ кадре, в том числе на кадре с тем же номером, который ядру не отдаётся (решение 109). None — биржа такого
    /// поля не даёт (прежнее значение затирается: версия должна быть именно из последнего кадра). Ошибок нет.
    pub fn frame_ver(&self, ticker: &str, id: Option<i64>, changed_ms: Option<i64>) {
        let now = now_ms();
        self.conn.alive.store(now, Ordering::Relaxed);
        if let Some(&i) = self.conn.idx.get(ticker) {
            let p = &self.conn.pairs[i];
            p.last.store(now, Ordering::Relaxed);
            p.id.store(id.unwrap_or(NO_VER), Ordering::Relaxed);
            p.changed
                .store(changed_ms.unwrap_or(NO_VER), Ordering::Relaxed);
        }
    }

    /// Пара переподписана: прогрев пары — с этого момента. Ошибок нет.
    pub fn resubscribed(&self, ticker: &str) {
        if let Some(&i) = self.conn.idx.get(ticker) {
            self.conn.pairs[i].since.store(now_ms(), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FeedReg;

    #[test]
    fn absent_book_reports_current_subscription_session_and_first_frame() {
        let reg = FeedReg::default();
        let (ws, _commands) = reg.register("market-2", &["ACE".into()]);
        assert!(
            reg.missing_book("BTC")
                .contains("нет в текущем реестре подписок")
        );
        assert!(reg.missing_book("ACE").contains("сессия ещё не запущена"));
        ws.session_started();
        assert!(reg.missing_book("ACE").contains("первый кадр"));
        ws.frame("ACE");
        assert!(reg.missing_book("ACE").contains("кадр пары уже наблюдался"));
        ws.session_ended();
        let missing = reg.missing_book("ACE");
        assert!(missing.contains("market-2") && missing.contains("сессия не активна"));
        ws.session_started();
        assert!(reg.missing_book("ACE").contains("первый кадр"));
        reg.clear();
        assert!(
            reg.missing_book("ACE")
                .contains("нет в текущем реестре подписок")
        );
    }
}
