//! Файл: venues/xt/ws_book.rs
//! Что делает: стаканы одного соединения XT.com: снимок depth → память и ядру (тот же updateId — только время кадра,
//! решение 109); изменения depth_update → локальный стакан (local_book.rs) со снимком REST из очереди (snapshots.rs);
//! разрыв номеров — пара недостоверна, новый снимок не раньше venue.ws_reconnect_pause_ms (урок 12: без круга
//! ресинков). Ядру — без ожидания (try_send). Дополнение 10.11 к решениям 202/213 (Тигран: согласовано): при
//! переподписке пары сторожем ядра — сразу снимок REST из той же очереди (resub), ядро получает свежую книгу, не
//! дожидаясь кадра (тихая пара после переподписки кадра может не прислать).
//! Кто вызывает: ws_conn.rs.
//! От чего зависит: depth.rs, local_book.rs, snapshots.rs, book.rs, ws.rs, ctx.rs; изменения — ws_diff.rs.
//! Настройки: из ws.rs (Cfg).
//!
//! Официально: «Orderbook manage» https://doc.xt.com/docs/spot/WebSocket%20Public/OrderbookManage — буфер изменений, снимок REST,
//! правила номеров (local_book.rs); снимок depth@{symbol},{levels} — data.i updateId.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::time::Instant;

use crate::core::types::{Asset, OrderBook, PriceLevel};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::Ctx;
use super::depth::{self, Data};
use super::local_book::LocalBook;
use super::snapshots::{self, Done};
use super::ws::{Cfg, Stream, invalidate};

/// Состояние стаканов соединения.
pub struct Conn<'a> {
    pub(super) ctx: &'a Ctx,
    pub(super) name: &'a str,
    pub(super) books: &'a Books,
    pub(super) out: &'a mpsc::Sender<MarketEvent>,
    pub(super) by_symbol: HashMap<String, &'a Asset>,
    pub(super) alive: Alive,
    pub(super) levels: usize,
    /// Канал стакана (venue.book_stream): какой снимок REST брать при переподписке и как его применять.
    pub(super) stream: Stream,
    pub(super) resync_pause: Duration,
    /// Снимки depth: последний отданный ядру updateId по символу.
    pub(super) last: HashMap<String, i64>,
    /// Изменения depth_update: локальные стаканы, ждущие снимка пары, когда можно просить снимок снова.
    pub(super) local: HashMap<String, LocalBook>,
    pub(super) pending: HashSet<String>,
    pub(super) not_before: HashMap<String, Instant>,
    pub(super) snap_tx: mpsc::Sender<Done>,
    pub(super) dropped: u64,
}

impl<'a> Conn<'a> {
    /// Новое состояние соединения. Ошибок нет.
    pub fn new(
        ctx: &'a Ctx,
        name: &'a str,
        assets: &'a [(Asset, String)],
        books: &'a Books,
        out: &'a mpsc::Sender<MarketEvent>,
        cfg: &Cfg,
        snap_tx: mpsc::Sender<Done>,
    ) -> Self {
        Self {
            ctx,
            name,
            books,
            out,
            by_symbol: assets.iter().map(|(a, s)| (s.to_lowercase(), a)).collect(),
            alive: Alive::default(),
            levels: cfg.levels as usize,
            stream: cfg.stream,
            resync_pause: cfg.resync_pause,
            last: HashMap::new(),
            local: HashMap::new(),
            pending: HashSet::new(),
            not_before: HashMap::new(),
            snap_tx,
            dropped: 0,
        }
    }

    /// Пришёл кадр — соединение живо. Ошибок нет.
    pub fn alive(&self) {
        self.alive
            .store(Utc::now().timestamp_millis(), Ordering::Relaxed);
    }

    /// Решение 213: номер стакана пары в памяти для учёта свежести (ConnHandle::frame_ver). Только depth_update —
    /// номер локального стакана (local_book.rs version: та же шкала, что lastUpdateId REST по «Orderbook manage»);
    /// снимки depth — None: связи updateId кадра depth@ с lastUpdateId REST в документации нет. Ошибок нет.
    pub fn version(&self, symbol: &str) -> Option<i64> {
        self.local.get(symbol).and_then(LocalBook::version)
    }

    /// Решение 202: пара переподписана — прежнее состояние её потока не продолжается. Снимки depth: первый снимок после
    /// подписки — ядру, даже с тем же updateId (решение 109); depth_update: локальный стакан сброшен, без паузы разрыва.
    /// Дополнение 10.11: снимок REST GET /v4/public/depth (https://doc.xt.com/docs/spot/Market/GetDepthData) просится сразу, в общей
    /// очереди snapshots.rs (лимит «10/s/ip»): depth_update — стакан пары заново со снимка по «Orderbook manage»
    /// (https://doc.xt.com/docs/spot/WebSocket%20Public/OrderbookManage: изменения новой подписки копятся в буфер до снимка, затем
    /// применяются по номерам), номер стакана = lastUpdateId снимка; depth — верхние venue.depth_levels уровней снимка
    /// ядру (ws_diff.rs on_snapshot). Снимок уже в пути — второй не просится. Стакан в памяти до снимка не снимается:
    /// его свежесть — по времени кадра пары. Ошибок нет (сбой снимка — в итоге очереди).
    pub fn resub(&mut self, symbol: &str) {
        self.last.remove(symbol);
        self.local.remove(symbol);
        self.not_before.remove(symbol);
        if self.pending.insert(symbol.to_string()) {
            let limit = (self.stream == Stream::Depth).then_some(self.levels as u64);
            snapshots::request(self.ctx, symbol.to_string(), limit, self.snap_tx.clone());
        }
    }

    /// Монета по символу XT (регистр не важен). Ошибок нет.
    pub fn asset(&self, symbol: &str) -> Option<&'a Asset> {
        self.by_symbol.get(&symbol.to_lowercase()).copied()
    }

    /// Снимок depth. Возвращает: false — канал ядра закрыт. Ошибок нет (сбой разбора — в журнал).
    pub async fn on_depth(&mut self, symbol: &str, d: &Data<'_>) -> bool {
        let Some(asset) = self.asset(symbol) else {
            return true;
        };
        if self.last.get(symbol) == Some(&d.i) {
            self.books.touch(&asset.ticker);
            return true;
        }
        match depth::snapshot_levels(d) {
            Ok((bids, asks)) => {
                self.last.insert(symbol.to_string(), d.i);
                self.give(asset, bids, asks, d.t)
            }
            Err(why) => {
                self.last.remove(symbol);
                self.bad(asset, format!("стакан {symbol} не разобран: {why}"))
                    .await;
                true
            }
        }
    }

    /// Стакан → память и ядру. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub(super) fn give(
        &mut self,
        asset: &Asset,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        t: Option<i64>,
    ) -> bool {
        let ob = OrderBook {
            venue: self.ctx.venue(),
            asset: asset.clone(),
            bids,
            asks,
            exchange_time: t.and_then(chrono::DateTime::<Utc>::from_timestamp_millis),
            received_at: Utc::now(),
        };
        self.books.put(ob.clone(), self.alive.clone());
        match self.out.try_send(MarketEvent::Book(ob)) {
            Ok(()) => true,
            Err(TrySendError::Closed(_)) => false,
            Err(TrySendError::Full(_)) => {
                self.dropped += 1;
                if self.dropped.is_power_of_two() {
                    tracing::warn!(
                        conn = self.name,
                        dropped = self.dropped,
                        "ядро не успевает принимать стаканы XT"
                    );
                }
                true
            }
        }
    }

    /// Стакан пары недостоверен: из памяти, ядру, в журнал. Ошибок нет.
    pub(super) async fn bad(&self, asset: &Asset, why: String) {
        invalidate(self.ctx, self.books, self.out, &asset.ticker);
        let m = format!("WebSocket {}: {why}", self.name);
        self.ctx.log(Level::Warn, "venue.ws", m).await;
    }
}
