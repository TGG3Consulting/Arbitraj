//! Файл: venues/xt/backup_conn.rs
//! Что делает: решение 419, дополнение (ревью 419) — страховка быстрого стакана XT в соединении: кадр снимка depth@
//! (пара не на быстром — ядру как раньше; на быстром — запомнить и сверить, расхождение — на снимки), разрыв быстрого
//! стакана (ядру снимок, принятый после последнего изменения быстрого, или стакан пары снимается до следующего
//! снимка), счёт разрывов и журнал venue.fast_book (склейка решения 334). Правила и обоснование — шапка backup.rs.
//! Кто вызывает: ws_conn.rs (кадр depth@), ws_diff.rs (разрыв номеров, неразборчивый кадр).
//! От чего зависит: backup.rs (Backup, Snap), ws_book.rs (Conn), local_book.rs, depth.rs, ws.rs (invalidate),
//! gateway/lost_log.rs.
//! Настройки: venue.ws_reconnect_pause_ms (через ws.rs Cfg — пауза до нового снимка REST после расхождения);
//! log.events_lost_interval_s (вся система, необязательная, читает gateway/lost_log.rs): частота записи о разрывах.
//!
//! Официально: https://doc.xt.com/docs/spot/WebSocket%20Public/LimitedDepth, IncrementalDepth, OrderbookManage.

use std::sync::LazyLock;

use tokio::time::Instant;

use crate::core::types::Asset;
use crate::gateway::lost_log::{Lost, Note};

use super::backup::Snap;
use super::ctx::ID;
use super::depth::{self, Data};
use super::ws::invalidate;
use super::ws_book::Conn;

/// Вид записи журнала о разрывах быстрого стакана.
pub const KIND: &str = "venue.fast_book";

/// Текст записи журнала (склейка решения 334).
const WHAT: &str = "XT: быстрый стакан depth_update разорван или разошёлся со снимками depth@ — пара на снимках depth@ до пересинхронизации (разрывов)";

/// Счёт разрывов быстрого стакана XT с запуска (один адаптер XT на процесс).
static GAPS: LazyLock<Lost> = LazyLock::new(Lost::default);

impl Conn<'_> {
    /// Что делает: кадр снимка depth@. Без страховки (канал depth) и при паре не на быстром — как раньше (on_depth: тот
    /// же updateId — только время, решение 109). Пара на быстром: кадр ядру не отдаётся, запоминается и сверяется с
    /// верхом быстрого (Backup::observe); расхождение — быстрый сбрасывается (пересинхронизация не раньше
    /// venue.ws_reconnect_pause_ms), ядру этот снимок. Принимает: символ, данные. Возвращает: (канал ядра открыт, кадр
    /// стал стаканом пары или продлил его — отметить пару в учёте свежести, решение 202). Ошибок нет.
    pub async fn on_depth_frame(&mut self, symbol: &str, d: &Data<'_>) -> (bool, bool) {
        if !self.backup.is_fast(symbol) {
            return (self.on_depth(symbol, d).await, true);
        }
        let (Some(asset), Ok((bids, asks))) = (self.asset(symbol), depth::snapshot_levels(d))
        else {
            // Неразборчивый снимок при быстром стакане: ядру не нужен; цепочка «подряд» прерывается.
            self.backup.take_seen(symbol);
            return (true, false);
        };
        let (fb, fa) = self
            .local
            .get(symbol)
            .map(|l| l.top(self.levels))
            .unwrap_or_default();
        let snap = Snap {
            bids,
            asks,
            t: d.t,
            i: d.i,
        };
        if !self.backup.observe(symbol, snap, &fb, &fa) {
            return (true, false);
        }
        let why = format!(
            "{symbol}: верх двух снимков depth@ подряд не равен быстрому стакану, изменений между ними нет"
        );
        self.local
            .entry(symbol.to_string())
            .or_default()
            .reset(why.clone());
        self.not_before
            .insert(symbol.to_string(), Instant::now() + self.resync_pause);
        (self.fast_lost(symbol, asset, why).await, true)
    }

    /// Что делает: быстрый стакан пары разорван (номера, неразборчивый кадр) или разошёлся со снимками. Без страховки —
    /// как раньше: стакан недостоверен, Warn в журнал. Со страховкой, пара была на быстром: есть снимок depth@, принятый
    /// после последнего изменения быстрого, — ядру он (не старее отданного по порядку получения); нет — стакан
    /// снимается, следующий кадр depth@ уходит ядру сразу. Счёт разрывов и запись venue.fast_book не чаще
    /// log.events_lost_interval_s. Принимает: символ, монету, причину. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub(super) async fn fast_lost(&mut self, symbol: &str, asset: &Asset, why: String) -> bool {
        if !self.backup.on {
            self.bad(asset, why).await;
            return true;
        }
        let mut open = true;
        if self.backup.is_fast(symbol) {
            let seen = self.backup.take_seen(symbol);
            self.backup.forget(symbol);
            match seen {
                Some(s) => {
                    self.last.insert(symbol.to_string(), s.i);
                    open = self.give(asset, s.bids, s.asks, s.t);
                }
                None => {
                    self.last.remove(symbol);
                    invalidate(self.ctx, self.books, self.out, &asset.ticker);
                }
            }
        }
        let note = Note {
            kind: KIND,
            venue: Some(ID),
            what: WHAT,
            last: format!("{symbol}: {why}"),
        };
        GAPS.hit(&self.ctx.settings, &self.ctx.journal, note);
        open
    }
}
