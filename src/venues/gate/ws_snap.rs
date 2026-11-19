//! Файл: venues/gate/ws_snap.rs
//! Что делает: кадры канала снимков spot.order_book в сессии WebSocket Gate: учёт свежести пары с lastUpdateId
//! (решения 202, 213), кадр в окно (решение 125), отсев решения 109 (тот же или меньший lastUpdateId ядру не
//! отдаётся — только время кадра пары), снимок ядру — через fast_pick.rs (решение 421: пока ядро на быстром стакане
//! spot.obu, снимок придерживается как страховка); неразборчивый кадр и отказ подписки — стакан пары недостоверен
//! (если ядро не на быстром стакане), журнал.
//! Кто вызывает: ws_conn.rs.
//! От чего зависит: depth.rs (Msg, to_book), fast_pick.rs, fast_io.rs (Env), ws.rs (invalidate), gateway/feed_reg.rs,
//! gateway/tap.rs.
//! Настройки: нет.
//!
//! Официально (Spot WebSocket v4, https://www.gate.com/docs/developers/apiv4/ws/en/): spot.order_book — result {t,
//! lastUpdateId, s, bids, asks}, снимок уровней; ответ на подписку — error = null или код ошибки. Основание решения
//! 109 и шкалы lastUpdateId = id REST — в шапках ws_conn.rs и book_version.rs.

use std::collections::HashMap;

use crate::gateway::core_out_book::Fed;
use crate::gateway::feed_reg::ConnHandle;
use crate::gateway::tap::{TapData, TapStream};
use crate::storage::journal::Level;

use super::ctx::ID;
use super::depth::{self, Msg};
use super::fast_io::Env;
use super::fast_pick::Fast;
use super::ws::invalidate;

/// Состояние снимков соединения: последний отданный (учтённый) lastUpdateId по символу (решение 109).
#[derive(Default)]
pub struct Snaps {
    pub last: HashMap<String, i64>,
}

impl Snaps {
    /// Что делает: кадр канала снимков (кроме spot.system upgrade — его разбирает ws_conn.rs). Принимает: окружение,
    /// быстрые стаканы, учёт свежести соединения, разобранный кадр, текст кадра (для окна), время приёма (нс).
    /// Возвращает: Some(Fed) — стакан ушёл ядру; None — отдавать нечего. Ошибок нет.
    pub async fn on_msg(
        &mut self,
        env: &Env<'_>,
        fast: &mut Fast,
        feed: &ConnHandle,
        m: Msg,
        text: &str,
        rx_ns: i64,
    ) -> Option<Fed> {
        match m {
            Msg::Depth {
                symbol,
                update_id,
                bids,
                asks,
                exchange_time,
            } => {
                let asset = *env.by_symbol.get(symbol.as_str())?;
                feed.frame_ver(&asset.ticker, Some(update_id), None);
                let push = || TapData::Text(text.to_string());
                env.ctx
                    .taps
                    .push(ID, &asset.ticker, TapStream::Book, env.name, rx_ns, push);
                if self.last.get(&symbol).is_some_and(|&id| update_id <= id) {
                    env.books.touch(&asset.ticker);
                    return None;
                }
                let book = depth::to_book(asset.clone(), bids, asks, exchange_time);
                let given = fast.snapshot(&symbol, book);
                self.last.insert(symbol, update_id);
                given.map(|b| env.give(b))
            }
            Msg::Bad { symbol, reason } => {
                if let Some(s) = symbol.as_deref()
                    && let Some(a) = env.by_symbol.get(s)
                {
                    self.lost(env, fast, s, &a.ticker);
                }
                env.log(Level::Error, format!("кадр не разобран: {reason}"))
                    .await;
                None
            }
            Msg::Ack {
                id,
                error: Some(err),
            } => {
                if let Some((a, s)) = id.and_then(|i| env.assets.get(i as usize)) {
                    self.lost(env, fast, s, &a.ticker);
                }
                env.log(Level::Error, format!("отказ подписки (id {id:?}): {err}"))
                    .await;
                None
            }
            _ => None,
        }
    }

    /// Снимки пары сбились: стакан ядра снимается, если ядро не на быстром стакане (решение 421: сбой страховки
    /// быстрый стакан не снимает); отсев решения 109 пары — заново. Ошибок нет.
    fn lost(&mut self, env: &Env<'_>, fast: &Fast, symbol: &str, ticker: &str) {
        if !fast.on_fast(symbol) {
            invalidate(env.ctx, env.books, env.out, ticker);
        }
        self.last.remove(symbol);
    }
}
