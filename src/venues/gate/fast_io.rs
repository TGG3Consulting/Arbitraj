//! Файл: venues/gate/fast_io.rs
//! Что делает: решение 421 — исполнение решений fast_pick.rs в сессии WebSocket Gate: стакан ядру (память котировки
//! book.rs + канал ядра решения 324), снятие стакана пары, кадры spot.obu (снимок, изменения, неразборчивый кадр,
//! отказ подписки), счёт разрывов быстрого стакана с записью журнала venue.fast_book.
//! Кто вызывает: ws_conn.rs.
//! От чего зависит: fast_pick.rs, obu.rs, book.rs, ws.rs (invalidate), gateway/core_out_book.rs, gateway/lost_log.rs.
//! Настройки: log.events_lost_interval_s (вся система, необязательная; читает gateway/lost_log.rs): задана — запись о
//! разрывах не чаще раза в столько секунд с числом разрывов за это время; не задана — на 1-м, 2-м, 4-м, 8-м… разрыве.
//!
//! Журнал (общая постановка решений 420–424, п. 5): каждый разрыв = переключение пары «быстрый → снимки» и одна
//! пересинхронизация потока (возврат на быстрый — следующий полный снимок spot.obu). Строка — существующим механизмом
//! склейки потерь (решение 334), без новой тяжёлой записи: «всего с запуска» и «за интервал» видны в «Журнале».
//! Отказ подписки spot.obu (Spot WebSocket v4: ответ с error ≠ null) — Error один раз на пару за подключение.

use std::collections::HashMap;
use std::sync::LazyLock;

use tokio::sync::mpsc::Sender;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;
use crate::gateway::lost_log::{Lost, Note};
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::fast_pick::{Fast, Give};
use super::obu::{self, Obu};
use super::ws::invalidate;

/// Вид записи журнала о разрывах быстрого стакана.
pub const KIND: &str = "venue.fast_book";

/// Счёт разрывов быстрого стакана Gate с запуска (один адаптер Gate на процесс).
static GAPS: LazyLock<Lost> = LazyLock::new(Lost::default);

/// Окружение сессии для отдачи стаканов.
pub struct Env<'a> {
    pub ctx: &'a Ctx,
    pub name: &'a str,
    pub books: &'a Books,
    pub out: &'a Sender<MarketEvent>,
    pub kept: &'a BookOut,
    pub alive: &'a Alive,
    pub by_symbol: &'a HashMap<&'a str, &'a Asset>,
    /// Пары соединения (монета, символ Gate): номер пары — id запроса подписки снимков.
    pub assets: &'a [(Asset, String)],
}

impl Env<'_> {
    /// Стакан ядру: в память котировки и в канал ядра (решение 324). Возвращает: Fed. Ошибок нет.
    pub fn give(&self, book: OrderBook) -> Fed {
        self.books.put(book.clone(), self.alive.clone());
        self.kept.book(self.out, book)
    }

    /// Исполняет решение fast_pick.rs. Принимает: тикер монеты, решение. Возвращает: Fed (Sent — если отдавать нечего).
    pub fn apply(&self, ticker: &str, g: Give) -> Fed {
        match g {
            Give::Book(b) => self.give(b),
            Give::Drop => {
                invalidate(self.ctx, self.books, self.out, ticker);
                Fed::Sent
            }
            Give::Nothing => Fed::Sent,
        }
    }

    /// Что делает: кадр канала spot.obu. Принимает: быстрые стаканы соединения, разобранный кадр. Возвращает: Fed.
    /// Ошибок нет: неизвестная пара — пропуск.
    pub async fn on_obu(&self, fast: &mut Fast, m: Obu) -> Fed {
        let stream = match &m {
            Obu::Update(f) => Some(f.stream.clone()),
            Obu::Ack { stream, .. } | Obu::Bad { stream, .. } => stream.clone(),
        };
        let Some(sym) = stream.as_deref().and_then(obu::symbol_of) else {
            if let Obu::Bad { reason, .. } = &m {
                self.log(
                    Level::Error,
                    format!("spot.obu: кадр без потока не разобран: {reason}"),
                )
                .await;
            }
            return Fed::Sent;
        };
        let Some(asset) = self.by_symbol.get(sym).copied() else {
            return Fed::Sent;
        };
        let g = match m {
            Obu::Update(f) => {
                let (g, gap) = fast.frame(sym, asset, &f);
                if let Some(why) = gap {
                    self.gap(sym, &why);
                }
                g
            }
            Obu::Bad { reason, .. } => {
                self.gap(sym, &format!("кадр не разобран: {reason}"));
                fast.lost(sym)
            }
            Obu::Ack {
                event,
                error: Some(err),
                ..
            } if event == "subscribe" => {
                let m = format!(
                    "spot.obu {sym}: подписка отклонена: {err}; пара — только на снимках spot.order_book до переподключения"
                );
                self.log(Level::Error, m).await;
                fast.refused(sym)
            }
            Obu::Ack {
                error: Some(err), ..
            } => {
                self.log(
                    Level::Warn,
                    format!("spot.obu {sym}: отписка отклонена: {err}"),
                )
                .await;
                Give::Nothing
            }
            Obu::Ack { .. } => Give::Nothing,
        };
        self.apply(&asset.ticker, g)
    }

    /// Счёт разрыва и, если пора, запись журнала. Принимает: символ, причину. Ошибок нет.
    fn gap(&self, sym: &str, why: &str) {
        let note = Note {
            kind: KIND,
            venue: Some(ID),
            what: "Gate: быстрый стакан spot.obu разорван — пара сразу на снимках spot.order_book, поток пересинхронизирован (разрывов)",
            last: format!("{sym}: {why}"),
        };
        GAPS.hit(&self.ctx.settings, &self.ctx.journal, note);
    }

    /// Запись в журнал (вид venue.ws). Ошибок нет.
    pub async fn log(&self, level: Level, msg: String) {
        let text = format!("WebSocket {}: {msg}", self.name);
        let _ = self
            .ctx
            .journal
            .write(level, "venue.ws", Some(ID), text, None)
            .await;
    }
}
