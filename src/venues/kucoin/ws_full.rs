//! Файл: venues/kucoin/ws_full.rs
//! Что делает: решение 423 — снимок REST Get Full OrderBook для быстрого стакана пары в сессии KuCoin: просьба снимка
//! в общей очереди (full_book.rs) — один на пару; после разрыва — не раньше venue.ws_reconnect_pause_ms; без темпа,
//! ключей или версии ключа снимок не просится (одна запись журнала на сессию, пары на снимках); итог снимка →
//! локальный стакан (local_book.rs snapshot: снимок + накопленные кадры) и дальше, как кадр (ws_fast.rs after).
//! Кто вызывает: ws_fast.rs (want_full), ws_conn.rs (on_full).
//! От чего зависит: ws_sess.rs (Sess), full_book.rs, local_book.rs, fast_log.rs, ws.rs (End).
//! Настройки: venue.full_book_per_sec, venue.api_key_version, ключи kucoin.* (full_book.rs ready).
//!
//! Официально: https://www.kucoin.com/docs-new/3470068w0 (калибровка: кэш кадров → снимок Get Full OrderBook →
//! проигрывание кэша).

use tokio::time::Instant;

use crate::core::types::Asset;
use crate::storage::journal::Level;

use super::fast_log::{self, Ev};
use super::full_book::{self, Done};
use super::local_book::Applied;
use super::ws::End;
use super::ws_sess::Sess;

impl Sess<'_> {
    /// Итог снимка REST из очереди. Возвращает: как on_incr. Ошибок нет.
    pub async fn on_full(&mut self, (symbol, res): Done) -> Option<End> {
        let f = self.fast.as_mut()?;
        f.pending.remove(&symbol);
        let asset = self.by_symbol.get(symbol.as_str()).copied()?;
        let lb = f.local.entry(symbol.clone()).or_default();
        let applied = match res {
            Ok(s) => lb.snapshot(s.seq, &s.bids, &s.asks),
            Err(why) => lb.reset(format!("снимок REST Full OrderBook не получен: {why}")),
        };
        if applied == Applied::Changed {
            fast_log::note(self.ctx, Ev::Resync, &asset.ticker, "снимок REST применён");
        }
        self.after(&symbol, asset, applied).await
    }

    /// Снимка нет — попросить его в очереди (один на пару; после разрыва — не раньше паузы; темп и ключи — ready).
    pub(super) async fn want_full(&mut self, symbol: &str, asset: &Asset) {
        let Some(f) = self.fast.as_mut() else { return };
        if f.pending.contains(symbol) {
            return;
        }
        let now = Instant::now();
        let early = f.not_before.get(symbol).is_some_and(|at| now < *at);
        let ready = if early {
            Ok(())
        } else {
            full_book::ready(self.ctx)
        };
        if early || ready.is_err() {
            if let Some(lb) = f.local.get_mut(symbol) {
                lb.clear_buffer();
            }
            if let Err(why) = ready {
                f.not_before.insert(symbol.to_string(), now + f.pause);
                if !std::mem::replace(&mut f.warned, true) {
                    let m = format!(
                        "WebSocket {}: быстрый стакан ждёт ({}: {why}) — стаканы пар из снимков",
                        self.name, asset.ticker
                    );
                    self.ctx.log(Level::Warn, "venue.book_fast", m).await;
                }
            }
            return;
        }
        f.pending.insert(symbol.to_string());
        full_book::request(self.ctx, symbol.to_string(), f.tx.clone());
    }
}
