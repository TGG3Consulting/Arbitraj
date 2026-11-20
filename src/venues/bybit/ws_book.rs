//! Файл: venues/bybit/ws_book.rs
//! Что делает: состояние одной сессии публичного WebSocket стаканов Bybit (основной поток или страховка, решение 428) и
//! разбор текстового кадра: стакан — ws_lane.rs; ответ op — отказ подписки делает стаканы её пачки недостоверными
//! (основной — сразу страховка, если есть; страховка — потеря страховки); неразобранный кадр — журнал. Возвращает
//! сессии, что отправить в соединение (пересподписки) и не пора ли закончить. Вынесено из ws_conn.rs (файл ≤ 200 строк).
//! Кто вызывает: ws_conn.rs.
//! От чего зависит: depth.rs, ws_lane.rs, ws_give.rs, local_book.rs, book.rs, ctx.rs, gateway/core_out_book.rs,
//! gateway/feed_reg.rs, gateway/tap.rs.
//! Настройки: raw.book_frames [bybit] — кадры стаканов в сырой журнал (решение 109; ответы op пишутся всегда).
//!
//! Официально (Connect https://bybit-exchange.github.io/docs/v5/ws/connect): ответ подписки спота —
//! {"success", "ret_msg", "req_id", "op":"subscribe"}; Orderbook — кадры orderbook.{depth}.{symbol}.

use std::collections::HashMap;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::core::types::{Asset, VenueId};
use crate::gateway::core_out_book::BookOut;
use crate::gateway::events::MarketEvent;
use crate::gateway::feed_reg::ConnHandle;
use crate::gateway::tap::{TapData, TapStream};
use crate::storage::journal::Level;

use super::backup::Main;
use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg, channel};
use super::local_book::LocalBook;
use super::ws::{Cfg, End};
use super::ws_give::{self, Insure};

/// Что сделать после кадра: отправить в соединение (тексты op) и/или закончить сессию.
#[derive(Default)]
pub struct Step {
    pub send: Vec<String>,
    pub end: Option<End>,
}

/// Состояние сессии.
pub struct St<'a> {
    pub ctx: &'a Ctx,
    pub name: &'a str,
    pub assets: &'a [(Asset, String)],
    pub books: &'a Books,
    pub out: &'a mpsc::Sender<MarketEvent>,
    pub cfg: &'a Cfg,
    pub ins: &'a Insure,
    /// Это соединение страховки (решение 428), а не основной поток.
    pub backup: bool,
    /// Учёт свежести для сторожа ядра (решение 202) — только у основного потока.
    pub feed: Option<&'a ConnHandle>,
    pub by_symbol: HashMap<&'a str, &'a Asset>,
    pub topics: Vec<String>,
    /// req_id → темы пачки подписки (чтобы снять их стаканы при отказе).
    pub sent: HashMap<String, Vec<String>>,
    pub local: HashMap<String, LocalBook>,
    /// Когда пара последний раз пересподписывалась после разрыва (второй разрыв раньше resync_gap — переподключение).
    pub resynced: HashMap<String, Instant>,
    pub alive: Alive,
    pub kept: BookOut,
    pub dropped: u64,
    pub venue: VenueId,
}

impl<'a> St<'a> {
    /// Новое состояние сессии. Принимает: окружение, имя соединения, пары, стаканы, канал ядра, настройки соединения,
    /// страховку, страховка ли это, учёт свежести (None у страховки). Ошибок нет.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &'a Ctx,
        name: &'a str,
        assets: &'a [(Asset, String)],
        books: &'a Books,
        out: &'a mpsc::Sender<MarketEvent>,
        cfg: &'a Cfg,
        ins: &'a Insure,
        feed: Option<&'a ConnHandle>,
    ) -> Self {
        Self {
            ctx,
            name,
            assets,
            books,
            out,
            cfg,
            ins,
            backup: feed.is_none(),
            feed,
            by_symbol: assets.iter().map(|(a, s)| (s.as_str(), a)).collect(),
            topics: assets.iter().map(|(_, s)| channel(cfg.levels, s)).collect(),
            sent: HashMap::new(),
            local: HashMap::new(),
            resynced: HashMap::new(),
            alive: Default::default(),
            kept: BookOut::new(out, ctx.venue(), &ctx.settings, &ctx.journal),
            dropped: 0,
            venue: VenueId(ID.to_string()),
        }
    }

    /// Текстовый кадр. Принимает: текст, время приёма (нс). Возвращает: Step. Ошибок нет.
    pub async fn on_text(&mut self, t: &str, rx_ns: i64) -> Step {
        match depth::parse(t) {
            Msg::Book {
                symbol,
                snapshot,
                u,
                seq,
                ts,
                bids,
                asks,
            } => {
                let Some(asset) = self.by_symbol.get(symbol).copied() else {
                    return Step::default();
                };
                if matches!(self.ctx.flag("raw.book_frames"), Ok(true)) {
                    self.ctx.raw.write(ID, self.name, t);
                }
                if !self.backup {
                    self.ctx.taps.push(
                        ID,
                        &asset.ticker,
                        TapStream::Book,
                        self.name,
                        rx_ns,
                        || TapData::Text(t.to_string()),
                    );
                }
                let f = super::ws_lane::Frame {
                    symbol,
                    snapshot,
                    u,
                    seq,
                    ts,
                    bids: &bids,
                    asks: &asks,
                };
                super::ws_lane::on_book(self, asset, f).await
            }
            Msg::Ack {
                op,
                success,
                msg,
                req_id,
            } => {
                self.ctx.raw.write(ID, self.name, t);
                if op == "subscribe" && !success {
                    self.refused(req_id, &msg).await;
                }
                Step::default()
            }
            Msg::Bad(reason) => {
                self.ctx.raw.write(ID, self.name, t);
                self.log(Level::Error, format!("кадр не разобран: {reason}"))
                    .await;
                Step::default()
            }
            Msg::Other => {
                self.ctx.raw.write(ID, self.name, t);
                Step::default()
            }
        }
    }

    /// Отказ подписки: стаканы пар пачки недостоверны (основной — сразу страховка, если есть). Ошибок нет.
    async fn refused(&mut self, req_id: Option<String>, msg: &str) {
        let batch = req_id
            .and_then(|id| self.sent.get(&id).cloned())
            .unwrap_or_else(|| self.topics.clone());
        for (a, s) in self.assets {
            if !batch.contains(&channel(self.cfg.levels, s)) {
                continue;
            }
            self.local.remove(s);
            let why = format!("отказ подписки: {msg}");
            if self.backup {
                ws_give::backup_lost(self.ctx, self.books, self.out, self.ins, &a.ticker, &why);
            } else {
                ws_give::main_lost(
                    self.ctx,
                    self.books,
                    self.out,
                    self.ins,
                    &a.ticker,
                    Main::Off,
                    &why,
                );
            }
        }
        self.log(
            Level::Error,
            format!("отказ подписки ({}): {msg}", batch.join(", ")),
        )
        .await;
    }

    /// Запись в журнал с именем соединения. Ошибок нет.
    pub async fn log(&self, level: Level, msg: String) {
        let _ = self
            .ctx
            .journal
            .write(
                level,
                "venue.ws",
                Some(ID),
                format!("WebSocket {}: {msg}", self.name),
                None,
            )
            .await;
    }
}
