//! Файл: venues/xt/backup.rs
//! Что делает: решение 419, дополнение (общая постановка решений 420–424) — страховка быстрого стакана XT. При
//! venue.book_stream = depth_update соединение слушает параллельно снимки depth@{symbol},{levels} (уровни —
//! venue.depth_levels, если она 5, 10, 20 или 50; иначе страховки нет — запись в журнал при подключении, ws.rs).
//! Ядру — локальный стакан depth_update (быстрый), пока он синхронизирован и отдан последним; снимки depth@ в это
//! время ядру не отдаются. Быстрого нет (старт до снимка REST, разрыв номеров, неразборчивый кадр, переподписка
//! сторожем) — ядру снимки depth@; быстрый пересинхронизируется прежней процедурой (снимок REST из очереди
//! snapshots.rs не раньше venue.ws_reconnect_pause_ms после разрыва), после неё — снова быстрый.
//! Кто вызывает: ws_conn.rs (кадр depth@), ws_diff.rs (разрыв, быстрый стакан отдан), ws_book.rs (переподписка).
//! От чего зависит: ws_book.rs (Conn), local_book.rs, depth.rs, ws.rs (invalidate), gateway/lost_log.rs.
//! Настройки: venue.depth_levels (через ws.rs Cfg); log.events_lost_interval_s (вся система, необязательная, читает
//! gateway/lost_log.rs): частота записи о разрывах.
//!
//! Официально (https://doc.xt.com/docs/spot/WebSocket%20Public/LimitedDepth, исходник `_websocket_public/limitDepth.md`):
//! «format: depth@{symbol},{levels}», «levels: 5, 10, 20, 50», «rate: 1000ms», data {s, i «updateId», t «time», a, b};
//! IncrementalDepth (`increDepth.md`): data {s, fi, i, a, b} — поля времени нет. Связи updateId снимка depth@ с
//! номерами depth_update и lastUpdateId REST документация не описывает (как и решение 213, ws_book.rs version).
//! «Не старее отданного» (постановка, п. 3): у быстрого стакана нет ни времени биржи, ни номера, сравнимого с
//! номером снимка, — сравнить можно только порядок получения кадров в одном соединении (оба потока — в нём).
//! Поэтому при разрыве последний принятый снимок НЕ отдаётся (он получен раньше последнего быстрого стакана — может
//! быть старее отданного): стакан пары снимается, и ядру сразу уходит следующий кадр depth@ (≤ 1000 мс). Возврат на
//! быстрый — стакан со снимка REST, полученного после снимка depth@. Сверки верхних уровней снимка с быстрым
//! стаканом (п. 4) нет: сопоставить кадры двух потоков по номеру или времени документация не позволяет.
//! Журнал (п. 5): каждый разрыв = переключение «быстрый → снимки» и пересинхронизация; счёт — механизмом склейки
//! потерь (решение 334), вид venue.fast_book; прежняя строка Warn на каждый разрыв в режиме страховки не пишется.

use std::collections::HashSet;
use std::sync::LazyLock;

use crate::core::types::Asset;
use crate::gateway::lost_log::{Lost, Note};

use super::ctx::ID;
use super::depth::Data;
use super::ws::invalidate;
use super::ws_book::Conn;

/// Вид записи журнала о разрывах быстрого стакана.
pub const KIND: &str = "venue.fast_book";

/// Счёт разрывов быстрого стакана XT с запуска (один адаптер XT на процесс).
static GAPS: LazyLock<Lost> = LazyLock::new(Lost::default);

/// Страховка одного соединения.
#[derive(Debug, Default)]
pub struct Backup {
    /// Страховка включена: depth_update и уровни снимков depth@ допустимы (ws.rs Cfg backup).
    pub on: bool,
    /// Пары, у которых ядру последним отдан быстрый стакан.
    fast: HashSet<String>,
}

impl Backup {
    /// Принимает: включена ли страховка. Ошибок нет.
    pub fn new(on: bool) -> Self {
        Self {
            on,
            fast: HashSet::new(),
        }
    }

    /// Быстрый стакан пары отдан ядру. Ошибок нет.
    pub fn given_fast(&mut self, symbol: &str) {
        if self.on && !self.fast.contains(symbol) {
            self.fast.insert(symbol.to_string());
        }
    }

    /// Пара больше не на быстром стакане (переподписка, сброс). Возвращает: была ли на быстром. Ошибок нет.
    pub fn forget(&mut self, symbol: &str) -> bool {
        self.fast.remove(symbol)
    }
}

impl Conn<'_> {
    /// Что делает: кадр снимка depth@. Без страховки (канал depth) — как раньше (on_depth). Со страховкой: пара на
    /// быстром стакане — кадр не отдаётся; иначе — ядру по правилам on_depth (тот же updateId — только время, решение
    /// 109). Принимает: символ, данные. Возвращает: (канал ядра открыт, кадр стал стаканом пары или продлил его —
    /// отметить пару в учёте свежести, решение 202). Ошибок нет.
    pub async fn on_depth_frame(&mut self, symbol: &str, d: &Data<'_>) -> (bool, bool) {
        if self.backup.on && self.backup.fast.contains(symbol) {
            return (true, false);
        }
        (self.on_depth(symbol, d).await, true)
    }

    /// Что делает: быстрый стакан пары разорван (номера, неразборчивый кадр). Без страховки — как раньше: стакан
    /// недостоверен, Warn в журнал. Со страховкой: пара была на быстром — стакан снимается (принятый снимок старее
    /// отданного, см. шапку), следующий кадр depth@ уходит ядру сразу; счёт разрывов и запись venue.fast_book не чаще
    /// log.events_lost_interval_s. Принимает: символ, монету, причину. Ошибок нет.
    pub(super) async fn fast_lost(&mut self, symbol: &str, asset: &Asset, why: String) {
        if !self.backup.on {
            self.bad(asset, why).await;
            return;
        }
        if self.backup.forget(symbol) {
            self.last.remove(symbol);
            invalidate(self.ctx, self.books, self.out, &asset.ticker);
        }
        let note = Note {
            kind: KIND,
            venue: Some(ID),
            what: "XT: быстрый стакан depth_update разорван — пара на снимках depth@ до пересинхронизации (разрывов)",
            last: format!("{symbol}: {why}"),
        };
        GAPS.hit(&self.ctx.settings, &self.ctx.journal, note);
    }
}
