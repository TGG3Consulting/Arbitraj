//! Файл: venues/mexc/ws_sess.rs
//! Что делает: состояние одной сессии WebSocket рынка MEXC и разбор пришедшего — вынесено из ws_conn.rs (решение
//! 420; логика снимков прежняя): каналы пар, стакан кадра → ядру (ws_out.rs) или в быстрый стакан (fast_pair.rs),
//! неразборчивый кадр и отказ подписки → стакан пары недостоверен, итоги очереди снимков REST быстрого стакана,
//! переподписка пары сторожем (каналы и сброс быстрого стакана). Быстрый стакан включается на сессию, если задана
//! venue.fast_book_interval, настройки очереди снимков в порядке (fast_snap.rs config) и подписок хватает: у MEXC не
//! больше 30 на соединение, у пары с быстрым стаканом их 2 (market.rs делит пары по 15). Иначе — строка журнала с
//! причиной, только снимки (ws_sess_ack.rs fast_on). Отказ подписки — ws_sess_ack.rs refused.
//! Кто вызывает: ws_conn.rs, ws_sess_ack.rs.
//! От чего зависит: depth.rs, ws_out.rs, fast_pair.rs, fast_snap.rs, book.rs, ws.rs (Cfg, invalidate), market.rs
//! (MAX_TOPICS_PER_CONN), gateway/feed_reg.rs.
//! Настройки: из ws.rs (Cfg); очередь снимков — fast_snap.rs.
//!
//! Официально: каналы — ws_conn.rs channel и fast_channel; «One ws connection supports a maximum of 30 subscriptions»
//! (https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/).

use std::collections::HashMap;

use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::gateway::feed_reg::ConnHandle;
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::Msg;
use super::fast_pair::Fast;
use super::fast_snap::Note;
use super::ws::{Cfg, invalidate};
use super::ws_conn::{channel, fast_channel};
use super::ws_out::{Gave, Out};

/// Сессия соединения.
pub struct Sess<'a> {
    pub(super) ctx: &'a Ctx,
    pub(super) name: &'a str,
    pub(super) books: &'a Books,
    pub(super) out: &'a mpsc::Sender<MarketEvent>,
    pub(super) feed: &'a ConnHandle,
    pub(super) assets: &'a [(Asset, String)],
    pub(super) by_symbol: HashMap<&'a str, &'a Asset>,
    pub(super) by_channel: HashMap<String, String>,
    /// Канал изменений → символ (решение 420); пусто — быстрого стакана нет.
    pub(super) fast_ch: HashMap<String, String>,
    /// Интервал канала изменений; None — быстрого стакана в сессии нет.
    pub fast_iv: Option<String>,
    pub(super) fast: Fast,
    pub give: Out<'a>,
    pub(super) levels: u64,
}

impl<'a> Sess<'a> {
    /// Что делает: состояние сессии. Принимает: окружение, имя, монеты (монета, символ), стаканы, канал ядра, учёт
    /// свежести, настройки, живость сессии. Возвращает: сессию и приёмник итогов очереди снимков. Ошибок нет.
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        ctx: &'a Ctx,
        name: &'a str,
        assets: &'a [(Asset, String)],
        books: &'a Books,
        out: &'a mpsc::Sender<MarketEvent>,
        feed: &'a ConnHandle,
        cfg: &Cfg,
        alive: Alive,
    ) -> (Self, mpsc::Receiver<Note>) {
        let fast_iv = super::ws_sess_ack::fast_on(ctx, name, cfg, assets.len()).await;
        let fast_ch = match &fast_iv {
            Some(iv) => assets
                .iter()
                .map(|(_, s)| (fast_channel(iv, s), s.clone()))
                .collect(),
            None => HashMap::new(),
        };
        let (tx, rx) = mpsc::channel(assets.len().max(1));
        let s = Self {
            ctx,
            name,
            books,
            out,
            feed,
            assets,
            by_symbol: assets.iter().map(|(a, s)| (s.as_str(), a)).collect(),
            by_channel: assets
                .iter()
                .map(|(a, s)| (channel(s, cfg.levels), a.ticker.clone()))
                .collect(),
            fast_ch,
            give: Out::new(ctx, out, books, alive, fast_iv.is_some()),
            fast_iv,
            fast: Fast::new(cfg.levels, tx),
            levels: cfg.levels,
        };
        (s, rx)
    }

    /// Каналы пары: снимки и (решение 420) изменения. Принимает: символ. Ошибок нет.
    pub fn channels(&self, symbol: &str) -> Vec<String> {
        let mut v = vec![channel(symbol, self.levels)];
        v.extend(self.fast_iv.as_deref().map(|iv| fast_channel(iv, symbol)));
        v
    }

    /// Монета по символу MEXC (для записи окна). Ошибок нет.
    pub fn asset(&self, symbol: &str) -> Option<&'a Asset> {
        self.by_symbol.get(symbol).copied()
    }

    /// Что делает: пара переподписана сторожем (каналы уже отправлены): следующий снимок — ядру, быстрый стакан пары —
    /// заново со снимка REST. Принимает: тикер. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub fn resubscribed(&mut self, ticker: &str) -> bool {
        let assets = self.assets;
        let Some((a, sym)) = assets.iter().find(|(a, _)| a.ticker == ticker) else {
            return true;
        };
        self.give.forget(Some(sym));
        if self.fast_iv.is_some() {
            let g = self.fast.drop_pair(
                self.ctx,
                &mut self.give,
                (a, sym),
                false,
                "переподписка сторожем пар",
            );
            return g != Some(Gave::Closed);
        }
        true
    }

    /// Что делает: сообщение очереди снимков REST (fast_snap.rs). Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub fn on_note(&mut self, note: Note) -> bool {
        let (Note::Sent(s) | Note::Done(s, _)) = &note;
        let asset = self.by_symbol.get(s.as_str()).copied();
        self.fast.on_note(self.ctx, &mut self.give, asset, note) != Some(Gave::Closed)
    }

    /// Что делает: разобранный кадр. Принимает: кадр. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub async fn on_msg(&mut self, msg: Msg) -> bool {
        let (ctx, name) = (self.ctx, self.name);
        let gave = match msg {
            Msg::Depth {
                symbol,
                bids,
                asks,
                exchange_time,
            } => {
                let Some(asset) = self.asset(&symbol) else {
                    return true;
                };
                self.feed.frame(&asset.ticker);
                // Решение 109: тот же снимок — ядру не отдаём, время кадра пары обновляется (решение 118, А-🟠8);
                // t79: стакан из ответа REST кадр не продлевает (ws_out.rs). Решение 420: при быстром стакане — fast_pair.rs.
                if self.fast_iv.is_some() {
                    self.fast.on_limit(
                        ctx,
                        &mut self.give,
                        asset,
                        &symbol,
                        bids,
                        asks,
                        exchange_time,
                    )
                } else {
                    Some(self.give.give(asset, &symbol, bids, asks, exchange_time))
                }
            }
            Msg::Diff { symbol, frame } => {
                let Some(asset) = self.asset(&symbol) else {
                    return true;
                };
                self.feed.frame(&asset.ticker);
                if self.fast_iv.is_none() {
                    return true;
                }
                self.fast
                    .on_diff(ctx, &mut self.give, asset, &symbol, frame)
            }
            Msg::Bad { symbol, reason } => {
                if let Some(a) = symbol.as_deref().and_then(|s| self.asset(s)) {
                    invalidate(ctx, self.books, self.out, &a.ticker);
                    // Стакан снят — следующий снимок обязательно отдать ядру, даже если он такой же.
                    self.give.forget(None);
                }
                log(
                    ctx,
                    Level::Error,
                    format!("WebSocket {name}: кадр не разобран: {reason}"),
                )
                .await;
                None
            }
            // Ответ на подписку с кодом не 0 — отказ (формата ошибки в документации нет: пишем как есть).
            Msg::Ack { code, msg } if code != 0 => return self.refused(code, msg).await,
            _ => None,
        };
        gave != Some(Gave::Closed)
    }
}

/// Запись в журнал. Ошибок нет.
pub async fn log(ctx: &Ctx, level: Level, msg: String) {
    let _ = ctx
        .journal
        .write(level, "venue.ws", Some(ID), msg, None)
        .await;
}
