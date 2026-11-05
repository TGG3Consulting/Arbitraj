//! Файл: venues/xt/ws_diff.rs
//! Что делает: изменения depth_update одного соединения XT.com → локальный стакан пары (local_book.rs): снимок REST
//! просится из общей очереди (snapshots.rs) один раз на пару, после разрыва — не раньше venue.ws_reconnect_pause_ms
//! (урок 12: без круга ресинков); итог снимка применяется к отложенным изменениям.
//! Кто вызывает: ws_conn.rs (через Conn из ws_book.rs).
//! От чего зависит: ws_book.rs (Conn), depth.rs, local_book.rs, snapshots.rs.
//! Настройки: из ws.rs (Cfg).
//!
//! Официально: «Orderbook manage» https://doc.xt.com/#websocket_publicorderBook.

use tokio::time::Instant;

use crate::core::types::Asset;

use super::depth::{self, Data};
use super::local_book::{Applied, Update};
use super::snapshots::{self, Done};
use super::ws_book::Conn;

impl<'a> Conn<'a> {
    /// Изменения depth_update. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub async fn on_update(&mut self, symbol: &str, d: &Data<'_>) -> bool {
        let Some(asset) = self.asset(symbol) else {
            return true;
        };
        let (bids, asks) = match (depth::convert(&d.b, "bids"), depth::convert(&d.a, "asks")) {
            (Ok(b), Ok(a)) => (b, a),
            (Err(why), _) | (_, Err(why)) => {
                self.local
                    .entry(symbol.to_string())
                    .or_default()
                    .reset(why.clone());
                self.bad(asset, format!("изменения {symbol} не разобраны: {why}"))
                    .await;
                return true;
            }
        };
        let u = Update {
            first: d.fi.unwrap_or(d.i),
            last: d.i,
            bids,
            asks,
        };
        let applied = self.local.entry(symbol.to_string()).or_default().apply(u);
        self.after(symbol, asset, applied, d.t).await
    }

    /// Итог снимка REST. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub async fn on_snapshot(&mut self, (symbol, res): Done) -> bool {
        self.pending.remove(&symbol);
        let Some(asset) = self.asset(&symbol) else {
            return true;
        };
        let lb = self.local.entry(symbol.clone()).or_default();
        let applied = match res {
            Ok(s) => lb.snapshot(s.id, &s.bids, &s.asks),
            Err(why) => lb.reset(format!("снимок REST не получен: {why}")),
        };
        self.after(&symbol, asset, applied, None).await
    }

    /// Что делать после изменения локального стакана. Возвращает: false — канал ядра закрыт. Ошибок нет.
    async fn after(&mut self, symbol: &str, asset: &'a Asset, a: Applied, t: Option<i64>) -> bool {
        match a {
            Applied::Changed => {
                let (bids, asks) = self
                    .local
                    .get(symbol)
                    .map(|l| l.top(self.levels))
                    .unwrap_or_default();
                self.give(asset, bids, asks, t)
            }
            Applied::Repeat => {
                self.books.touch(&asset.ticker);
                true
            }
            Applied::Waiting => {
                let allowed = self
                    .not_before
                    .get(symbol)
                    .is_none_or(|at| Instant::now() >= *at);
                if allowed && self.pending.insert(symbol.to_string()) {
                    snapshots::request(self.ctx, symbol.to_string(), self.snap_tx.clone());
                }
                true
            }
            Applied::Gap(why) => {
                self.not_before
                    .insert(symbol.to_string(), Instant::now() + self.resync_pause);
                let m =
                    format!("{symbol} — {why}; новый снимок не раньше venue.ws_reconnect_pause_ms");
                self.bad(asset, m).await;
                true
            }
        }
    }
}
