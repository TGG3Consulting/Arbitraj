//! Файл: venues/mexc/ws_sess_ack.rs
//! Что делает: сессия WebSocket рынка MEXC (ws_sess.rs) — отказ подписки и включение быстрого стакана (решение 420):
//! отказ канала снимков — стакан пары недостоверен (как раньше); отказ канала изменений — у пары только снимки до конца
//! сессии (стакан не снимается). Быстрый стакан на сессию — если задана venue.fast_book_interval, настройки очереди
//! снимков в порядке (fast_snap.rs config) и подписок хватает (2 на пару, предел MEXC 30 на соединение); иначе строка
//! журнала venue.ws с причиной.
//! Кто вызывает: ws_sess.rs.
//! От чего зависит: ws_sess.rs (Sess), fast_pair.rs, fast_snap.rs, market.rs (пределы подписок), ws.rs (Cfg,
//! invalidate), ws_out.rs.
//! Настройки: venue.fast_book_interval (через ws.rs Cfg); очередь снимков — fast_snap.rs.
//!
//! Официально: «One ws connection supports a maximum of 30 subscriptions»
//! (https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/); формата ответа-отказа в документации нет —
//! канал ищется в тексте msg, как раньше (ws_conn.rs).

use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::market::{MAX_TOPICS_PER_CONN, TOPICS_PER_FAST_PAIR};
use super::ws::{Cfg, invalidate};
use super::ws_out::Gave;
use super::ws_sess::{Sess, log};

impl Sess<'_> {
    /// Отказ подписки: канал снимков — стакан пары недостоверен; решение 420: канал изменений — у пары только снимки до
    /// конца сессии (стакан не снимается). Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub(super) async fn refused(&mut self, code: i64, msg: String) -> bool {
        let ctx = self.ctx;
        let fast: Vec<String> = self
            .fast_ch
            .iter()
            .filter(|(ch, _)| msg.contains(ch.as_str()))
            .map(|(_, s)| s.clone())
            .collect();
        for sym in fast {
            let Some(a) = self.asset(&sym) else { continue };
            let why = format!("отказ подписки канала изменений, код {code}");
            if self
                .fast
                .drop_pair(ctx, &mut self.give, (a, &sym), true, &why)
                == Some(Gave::Closed)
            {
                return false;
            }
        }
        for (ch, ticker) in &self.by_channel {
            if msg.contains(ch.as_str()) {
                invalidate(ctx, self.books, self.out, ticker);
                self.give.forget(None);
            }
        }
        log(
            ctx,
            Level::Error,
            format!("WebSocket {}: отказ, код {code}: {msg}", self.name),
        )
        .await;
        true
    }
}

/// Решение 420: быстрый стакан в этой сессии (условия — шапка): интервал или None и строка журнала. Ошибок нет.
pub async fn fast_on(ctx: &Ctx, name: &str, cfg: &Cfg, pairs: usize) -> Option<String> {
    let interval = cfg.fast.clone()?;
    let (max, per) = (MAX_TOPICS_PER_CONN as usize, TOPICS_PER_FAST_PAIR as usize);
    let why = match super::fast_snap::config(ctx) {
        Err(e) => e,
        Ok(_) if pairs * per > max => format!(
            "{pairs} пар — {} подписок, предел MEXC {max} на соединение",
            pairs * per
        ),
        Ok(_) => return Some(interval),
    };
    let m =
        format!("WebSocket {name}: быстрый стакан выключен в этой сессии (только снимки): {why}");
    log(ctx, Level::Error, m).await;
    None
}
