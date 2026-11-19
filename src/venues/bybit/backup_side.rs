//! Файл: venues/bybit/backup_side.rs
//! Что делает: решение 428 — сторона страховки в выборе источника стакана (Hub, backup.rs): кадр страховки (отдать ядру,
//! пока основной не синхронизирован; сверка при равном seq — расхождение переводит стакан ядра на страховку и просит
//! основное соединение переподписать пару), повтор снимка с тем же u (время кадра), потеря страховки (снять стакан
//! ядра, если он был из неё). Вынесено из backup.rs (файл ≤ 200 строк).
//! Кто вызывает: ws_lane.rs, ws_give.rs.
//! От чего зависит: backup.rs (Hub, Slot), backup_kind.rs.
//! Настройки: нет.
//!
//! Официально (Orderbook https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook): seq — «You can use this
//! field to compare different levels orderbook data, and for the smaller seq, then it means the data is generated
//! earlier».

use std::sync::atomic::Ordering;
use std::time::Instant;

use super::backup::{Hub, newer};
use super::backup_kind::{BackupAct, Held, Main, Print};

impl Hub {
    /// Кадр страховки применён. Принимает: тикер, стакан страховки, отпечаток, отдачу ядру (под замком пары, при Give и
    /// Mismatch). Возвращает: BackupAct. Ошибок нет.
    pub fn backup_frame(
        &self,
        ticker: &str,
        held: Held,
        p: Print,
        give: impl FnOnce(&Held),
    ) -> BackupAct {
        self.with(ticker, |s| {
            let seq = held.seq;
            s.backup = Some(held);
            s.backup_print = Some(p);
            if s.main == Main::Synced {
                let Some((m, mp)) = s.main_at else {
                    return BackupAct::Keep;
                };
                if m != seq || mp.0 != p.0 {
                    return BackupAct::Keep;
                }
                self.compares.fetch_add(1, Ordering::Relaxed);
                if mp == p {
                    return BackupAct::Keep;
                }
                // Расхождение при равном seq: стакан того же момента — страховки (seq равен отданному — замена).
                s.main = Main::Resync;
                s.main_at = None;
                s.given = Some(s.given.map_or(seq, |g| g.max(seq)));
                s.from_backup = true;
                if let Some(h) = &s.backup {
                    give(h);
                }
                return BackupAct::Mismatch(s.conn.clone());
            }
            if !newer(seq, s.given) {
                return BackupAct::Keep;
            }
            s.given = Some(seq);
            s.from_backup = true;
            if let Some(h) = &s.backup {
                give(h);
            }
            BackupAct::Give
        })
    }

    /// Кадр страховки без изменений (повтор снимка с тем же u): время кадра. Возвращает: стакан ядра из страховки.
    /// Ошибок нет.
    pub fn backup_touch(&self, ticker: &str) -> bool {
        self.with(ticker, |s| {
            if let Some(b) = s.backup.as_mut() {
                b.at = Instant::now();
            }
            s.from_backup
        })
    }

    /// Страховка пары потеряна (разрыв, обрыв, отказ подписки). Принимает: тикер, снятие стакана пары (вызывается под
    /// замком, если стакан ядра был из страховки — он недостоверен). Возвращает: был ли снят. Ошибок нет.
    pub fn backup_lost(&self, ticker: &str, invalidate: impl FnOnce()) -> bool {
        self.with(ticker, |s| {
            s.backup = None;
            s.backup_print = None;
            let was = std::mem::take(&mut s.from_backup);
            if was {
                invalidate();
            }
            was
        })
    }
}
