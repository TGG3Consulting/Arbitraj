//! Файл: venues/kucoin/book_rest.rs
//! Что делает: t79 (Тигран 10.11: согласовано; дополнение к решениям 202/213) — ответ REST сторожа ядра стаканом тихой
//! пары KuCoin (контракт book_from_rest). Пара по содержимому признана «книга не менялась с переподписки»
//! (core/feed_rest_ref.rs): первые venue.depth_levels уровней ответа Get Part OrderBook level2_{size} (book_print.rs — тот же
//! запрос и разбор, что у подтверждения тихой пары; столько же уровней даёт кадр /spotMarket/level2Depth5|50) кладутся
//! стаканом пары, как кадр:
//! - время кадра пары — получение ответа (recv_ms), подтверждения тихой пары нет — freeze (book.rs) отдаёт стакан не
//!   дольше venue.book_stale_ms от ответа, живость соединения (feed.stale_after_ms) обязательна, как всегда;
//! - кадр пары пришёл после отправки запроса (sent_ms) — не ставится: стакан потока новее;
//! - стакана пары нет — только если пара подписана в текущей сессии соединения (bind; живость — этой сессии), иначе
//!   не ставится (стакан снят: обрыв, отказ подписки, кадр не разобран);
//! - следующий кадр потока кладётся как обычно (put — ws_conn.rs кладёт каждый кадр) и заменяет снимок.
//!
//! Кто вызывает: mod.rs (book_from_rest), ws_conn.rs (bind — начало сессии).
//! От чего зависит: book.rs (Books, Entry), gateway/rest_book.rs (from_print), gateway/feed_health.rs (BookPrint).
//! Настройки: нет своих (свежесть — book.rs).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use crate::core::types::{Asset, OrderBook, VenueId};
use crate::gateway::feed_health::BookPrint;

use super::book::{Alive, Books};

impl Books {
    /// Пары соединения подписаны в новой сессии: монета и живость этой сессии (стакан из ответа REST — у пары без
    /// кадров в сессии). Снимает drop_ticker. Принимает: монеты соединения (монета, символ), живость. Ошибок нет.
    pub fn bind(&self, assets: &[(Asset, String)], alive: &Alive) {
        let mut b = self.bound.write().unwrap_or_else(|p| p.into_inner());
        for (a, _) in assets {
            b.insert(a.ticker.clone(), (a.clone(), alive.clone()));
        }
    }

    /// Ставит уровни ответа REST стаканом пары (правила — в шапке). Принимает: площадку, тикер KuCoin, уровни, время
    /// отправки запроса и получения ответа (unix мс, часы бота). Возвращает: поставленный стакан.
    /// Ошибка — текст причины: кадр пары новее запроса; пара не подписана в живой сессии; время вне диапазона.
    pub fn put_rest(
        &self,
        venue: VenueId,
        ticker: &str,
        levels: &BookPrint,
        sent_ms: i64,
        recv_ms: i64,
    ) -> Result<OrderBook, String> {
        let mut m = self.map.write().unwrap_or_else(|p| p.into_inner());
        let (asset, alive) = match m.get(ticker) {
            Some((b, alive, seen, ..)) => {
                let s = seen.load(Ordering::Relaxed);
                if s > sent_ms {
                    return Err(format!(
                        "кадр пары пришёл через {} мс после отправки запроса REST — стакан потока новее",
                        s - sent_ms
                    ));
                }
                (b.asset.clone(), alive.clone())
            }
            None => self
                .bound
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .get(ticker)
                .cloned()
                .ok_or_else(|| {
                    "стакана пары нет и пара не подписана в живой сессии соединения (стакан снят)"
                        .to_string()
                })?,
        };
        let book = crate::gateway::rest_book::from_print(venue, asset, levels, recv_ms)?;
        let seen = Arc::new(AtomicI64::new(recv_ms));
        let until = Arc::new(AtomicI64::new(0));
        m.insert(ticker.to_string(), (book.clone(), alive, seen, until, true));
        Ok(book)
    }
}
