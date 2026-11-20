//! Файл: venues/xt/backup.rs
//! Что делает: решение 419, дополнение (общая постановка решений 420–424) — страховка быстрого стакана XT. При
//! venue.book_stream = depth_update соединение слушает параллельно снимки depth@{symbol},{levels} (уровни —
//! venue.depth_levels, если она 5, 10, 20 или 50; иначе страховки нет — запись в журнал при подключении, ws.rs).
//! Ядру — локальный стакан depth_update (быстрый), пока он синхронизирован и отдан последним; снимки depth@ в это
//! время ядру не отдаются. Быстрого нет (старт до снимка REST, разрыв номеров, неразборчивый кадр, переподписка
//! сторожем) — ядру снимки depth@; быстрый пересинхронизируется прежней процедурой (снимок REST из очереди
//! snapshots.rs не раньше venue.ws_reconnect_pause_ms после разрыва), после неё — снова быстрый.
//! Здесь — учёт пар (Backup) и сверка верха (same_top); работа с соединением (кадр depth@, разрыв) — backup_conn.rs.
//! Кто вызывает: backup_conn.rs, ws_diff.rs (быстрый стакан отдан), ws_book.rs (переподписка, Conn), тесты.
//! От чего зависит: core/types (PriceLevel).
//! Настройки: venue.depth_levels (через ws.rs Cfg); log.events_lost_interval_s (вся система, необязательная, читает
//! gateway/lost_log.rs): частота записи о разрывах.
//!
//! Официально (https://doc.xt.com/docs/spot/WebSocket%20Public/LimitedDepth, исходник `_websocket_public/limitDepth.md`):
//! «format: depth@{symbol},{levels}», «levels: 5, 10, 20, 50», «rate: 1000ms», data {s, i «updateId», t «time», a, b};
//! IncrementalDepth (`increDepth.md`): data {s, fi, i, a, b} — поля времени нет. Ревью 419: пример на doc.xt.com
//! LimitedDepth (19.11.2026) другой — {s, i: 1657699200000, a, b}, без t (t в depth.rs необязательно; docs/TASKS.md). Связи updateId снимка depth@ с
//! номерами depth_update и lastUpdateId REST документация не описывает (как и решение 213, ws_book.rs version).
//! «Не старее отданного» (постановка, п. 3): у быстрого стакана нет ни времени биржи, ни номера, сравнимого с
//! номером снимка, — сравнить можно только порядок получения кадров в одном соединении (оба потока — в нём).
//! Поэтому при разрыве ядру уходит последний снимок depth@ пары, только если он получен ПОСЛЕ последнего изменения
//! быстрого стакана (ревью 419: «seen» — кадр принят, быстрый с тех пор не менялся); иначе стакан пары снимается, и
//! ядру сразу уходит следующий кадр depth@ (≤ 1000 мс). Возврат на быстрый — стакан со снимка REST, полученного после
//! снимка depth@. Сверка (п. 4, ревью 419, как 420/424 ревью): номерами и временем кадры потоков не сопоставить, но
//! по порядку получения — можно: два кадра depth@ ПОДРЯД не равны верху быстрого стакана (same_top), а быстрый между
//! ними и после первого не менялся (ни одного применённого изменения depth_update) — оба снимка новее отданного, и
//! быстрый молчит о реальном изменении дольше периода снимков (1000 мс против 100 мс depth_update) — расхождение:
//! ядру этот снимок, быстрый сбрасывается и пересинхронизируется (не раньше venue.ws_reconnect_pause_ms). Нового числа
//! нет. Иначе неверный быстрый стакан при целой цепочке fi/i (например, уровень вне снимка REST 500, вошедший в верх —
//! «Orderbook manage», Note) держался бы, пока не изменятся именно эти уровни.
//! Журнал (п. 5): каждый разрыв и расхождение = переключение «быстрый → снимки» и пересинхронизация; счёт — механизмом
//! склейки потерь (решение 334), вид venue.fast_book; прежняя строка Warn на каждый разрыв в режиме страховки не пишется.

use std::collections::{HashMap, HashSet};

use crate::core::types::PriceLevel;

/// Кадр снимка depth@ в числах ядра (уровни отсортированы, нулевые убраны — depth.rs snapshot_levels).
#[derive(Debug, Clone)]
pub struct Snap {
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    /// Время кадра (t), если есть.
    pub t: Option<i64>,
    /// updateId кадра (i) — решение 109: тот же i потом только продлевает.
    pub i: i64,
}

/// Последний снимок пары на быстром стакане, принятый после последнего изменения быстрого.
#[derive(Debug)]
struct Seen {
    snap: Snap,
    /// Верх снимка не равен верху быстрого стакана.
    differs: bool,
}

/// Страховка одного соединения.
#[derive(Debug, Default)]
pub struct Backup {
    /// Страховка включена: depth_update и уровни снимков depth@ допустимы (ws.rs Cfg backup).
    pub on: bool,
    /// Пары, у которых ядру последним отдан быстрый стакан.
    fast: HashSet<String>,
    /// Пары на быстром: снимок, принятый после последнего изменения быстрого (ревью 419). Не больше одного на пару.
    seen: HashMap<String, Seen>,
}

/// Верх снимка совпадает с верхом быстрого стакана одной стороны: общие верхние уровни (сколько есть у обоих) равны
/// по цене и количеству (Decimal — «1.20» = «1.2»), и сторона пуста у обоих или ни у кого. Разная длина сама по себе
/// не расхождение (снимок REST venue.depth_snapshot_limit может быть мельче venue.depth_levels). Ошибок нет.
pub fn same_top(snap: &[PriceLevel], fast: &[PriceLevel]) -> bool {
    let k = snap.len().min(fast.len());
    snap[..k] == fast[..k] && snap.is_empty() == fast.is_empty()
}

impl Backup {
    /// Принимает: включена ли страховка. Ошибок нет.
    pub fn new(on: bool) -> Self {
        Self {
            on,
            ..Self::default()
        }
    }

    /// Быстрый стакан пары изменился и отдан ядру: принятый раньше снимок теперь старее отданного. Ошибок нет.
    pub fn given_fast(&mut self, symbol: &str) {
        if !self.on {
            return;
        }
        if !self.fast.contains(symbol) {
            self.fast.insert(symbol.to_string());
        }
        self.seen.remove(symbol);
    }

    /// Пара больше не на быстром стакане (переподписка, сброс). Возвращает: была ли на быстром. Ошибок нет.
    pub fn forget(&mut self, symbol: &str) -> bool {
        self.seen.remove(symbol);
        self.fast.remove(symbol)
    }

    /// Пара на быстром стакане (ядру последним отдан он). Ошибок нет.
    pub fn is_fast(&self, symbol: &str) -> bool {
        self.on && self.fast.contains(symbol)
    }

    /// Что делает: кадр снимка depth@ пары на быстром стакане — запомнить и сверить с верхом быстрого (ревью 419).
    /// Принимает: символ, снимок, верх быстрого стакана (bids, asks). Возвращает: true — расхождение (этот и прежний
    /// снимок подряд не равны быстрому, быстрый между ними не менялся). Ошибок нет.
    pub fn observe(
        &mut self,
        symbol: &str,
        snap: Snap,
        fast_bids: &[PriceLevel],
        fast_asks: &[PriceLevel],
    ) -> bool {
        let differs = !same_top(&snap.bids, fast_bids) || !same_top(&snap.asks, fast_asks);
        let twice = differs && self.seen.get(symbol).is_some_and(|s| s.differs);
        self.seen.insert(symbol.to_string(), Seen { snap, differs });
        twice
    }

    /// Снимок пары, принятый после последнего изменения быстрого (забирается). Ошибок нет.
    pub fn take_seen(&mut self, symbol: &str) -> Option<Snap> {
        self.seen.remove(symbol).map(|s| s.snap)
    }
}
