//! Файл: venues/mexc/fast_resync.rs
//! Что делает: решение 420 — синхронизация быстрых стаканов MEXC (fast_pair.rs) снимком REST и страховка: снимок пары
//! в общую очередь (fast_snap.rs; один в пути на пару, после сбоя — не раньше venue.ws_reconnect_pause_ms), «запрос
//! ушёл» — отложенные до него кадры не нужны, итог снимка — в локальный стакан (fast_book.rs: шаги 4–7 процедуры
//! MEXC); быстрый стакан недостоверен — ядру снова поток снимков (последний кадр, если он свежий). Снимок, запрошенный
//! не первым для пары, — пересинхронизация (счётчик fast_stats.rs).
//! Кто вызывает: fast_pair.rs, ws_sess.rs (on_note).
//! От чего зависит: fast_pair.rs (Fast, Pair), fast_book.rs, fast_snap.rs, fast_stats.rs, ws_out.rs, ctx.rs.
//! Настройки: venue.ws_reconnect_pause_ms [mexc], venue.book_stale_ms [mexc] (свежесть кадра снимков при переключении).
//!
//! Официально: процедура «How to Properly Maintain a Local Copy of the Order Book» — fast_book.rs; лимиты — fast_snap.rs.

use std::time::Duration;

use chrono::Utc;
use tokio::time::Instant;

use crate::core::types::Asset;

use super::ctx::Ctx;
use super::fast_pair::Fast;
use super::fast_snap::{self, Note};
use super::fast_stats::{self, Ev};
use super::ws_out::{Gave, Out};

impl Fast {
    /// Что делает: сообщение очереди снимков. Принимает: окружение, отдачу, монету (None — пары нет в соединении),
    /// сообщение. Возвращает: итог отдачи; None — ядру ничего не отдавалось. Ошибок нет.
    pub fn on_note(
        &mut self,
        ctx: &Ctx,
        out: &mut Out<'_>,
        asset: Option<&Asset>,
        note: Note,
    ) -> Option<Gave> {
        let (symbol, res) = match note {
            Note::Sent(s) => {
                if let Some(p) = self.pairs.get_mut(&s) {
                    p.book.start_buffer();
                }
                return None;
            }
            Note::Done(s, r) => (s, r),
        };
        let asset = asset?;
        let p = self.pairs.entry(symbol.clone()).or_default();
        p.pending = false;
        let applied = match res {
            Ok(s) => p.book.snapshot(s.id, &s.bids, &s.asks),
            Err(why) => {
                let pause = ctx.int("venue.ws_reconnect_pause_ms").unwrap_or(0);
                p.retry_at = Some(Instant::now() + Duration::from_millis(pause));
                p.book.reset(String::new());
                fast_stats::note(&ctx.journal, Ev::SnapFail, &asset.ticker, &why);
                return None;
            }
        };
        self.after(ctx, out, asset, &symbol, applied)
    }

    /// Снимок REST пары в очередь, если его нет в пути и пауза после сбоя прошла. Ошибок нет.
    pub(super) fn ask(&mut self, ctx: &Ctx, asset: &Asset, symbol: &str) {
        let p = self.pairs.entry(symbol.to_string()).or_default();
        if p.pending || p.retry_at.is_some_and(|t| Instant::now() < t) {
            return;
        }
        p.pending = true;
        p.retry_at = None;
        if p.asked > 0 {
            fast_stats::note(
                &ctx.journal,
                Ev::Resync,
                &asset.ticker,
                "снимок REST в очереди",
            );
        }
        p.asked += 1;
        fast_snap::request(ctx, symbol.to_string(), self.tx.clone());
    }

    /// Быстрый стакан недостоверен (уже сброшен): ядру — снимки, последний кадр снимков — сейчас, если он не старее
    /// venue.book_stale_ms по времени получения (существующее правило свежести; кладётся с этим временем — ws_out.rs
    /// give_seen) и не старее отданного ядру (ws_out.rs). Иначе ядро держит последний быстрый стакан (верный на своё
    /// время) до следующего кадра снимков. Возвращает: итог отдачи. Ошибок нет.
    pub(super) fn fallback(
        &mut self,
        ctx: &Ctx,
        out: &mut Out<'_>,
        asset: &Asset,
        symbol: &str,
        why: &str,
    ) -> Option<Gave> {
        let p = self.pairs.entry(symbol.to_string()).or_default();
        if !p.fast {
            return None;
        }
        p.fast = false;
        fast_stats::note(&ctx.journal, Ev::ToSnap, &asset.ticker, why);
        let (b, a, t, rx_ms) = p.snap.clone()?;
        let limit = ctx.int("venue.book_stale_ms").ok()? as i64;
        if Utc::now().timestamp_millis() - rx_ms > limit {
            return None;
        }
        Some(out.give_seen(asset, symbol, (b, a), t, Some(rx_ms)))
    }
}
