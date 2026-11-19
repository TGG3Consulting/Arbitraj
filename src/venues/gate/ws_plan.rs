//! Файл: venues/gate/ws_plan.rs
//! Что делает: очередь запросов подписки одного соединения Gate: по одному запросу на шаг (темп venue.ws_subs_per_sec
//! задаёт ws_conn.rs): сначала подписки пар — у каждой пары снимки spot.order_book (страховка), затем, при
//! venue.obu_level (решение 421), поток spot.obu; после — пересинхронизация быстрых стаканов (unsubscribe + subscribe
//! потока пары из очереди fast_pick.rs). Запрос снимков пары — и для переподписки сторожем (решение 202).
//! Кто вызывает: ws_conn.rs.
//! От чего зависит: depth.rs (запрос spot.order_book), obu.rs (запрос spot.obu), fast_pick.rs (очередь), ws.rs (Cfg).
//! Настройки: из Cfg (venue.depth_levels, venue.book_interval, venue.obu_level).
//!
//! Официально (Spot WebSocket v4, https://www.gate.com/docs/developers/apiv4/ws/en/): spot.order_book — payload [пара,
//! уровни, интервал]; spot.obu — payload ["ob.{symbol}.{level}"], при разрыве номеров «you must unsubscribe from the
//! market and resubscribe to retrieve the initial depth snapshot». Предела подписок на соединение в документации нет
//! (вопрос в docs/TASKS.md) — подписок на пару при быстром стакане две.
//! Номер запроса (id): снимки — номер пары в соединении (ответ-отказ находит по нему пару, ws_conn.rs); spot.obu — номер
//! пары + число пар (ответ spot.obu находит пару по потоку в payload, fast_io.rs).

use serde_json::json;

use crate::core::types::Asset;

use super::depth::{self, BOOK_CHANNEL};
use super::fast_pick::Fast;
use super::obu;
use super::ws::Cfg;

/// Очередь подписок соединения.
pub struct Plan<'a> {
    assets: &'a [(Asset, String)],
    levels: u64,
    interval: String,
    /// Подписок на пару: 2 (снимки и spot.obu) или 1 (только снимки).
    per_pair: usize,
    /// Сколько первичных подписок уже отправлено.
    next: usize,
}

impl<'a> Plan<'a> {
    /// Принимает: пары соединения (монета, символ Gate), настройки соединения. Ошибок нет.
    pub fn new(assets: &'a [(Asset, String)], cfg: &Cfg) -> Self {
        Self {
            assets,
            levels: cfg.levels,
            interval: cfg.interval.clone(),
            per_pair: if cfg.obu.is_some() { 2 } else { 1 },
            next: 0,
        }
    }

    /// Есть ли что отправлять (первичные подписки или пересинхронизация). Ошибок нет.
    pub fn busy(&self, fast: &Fast) -> bool {
        self.next < self.assets.len() * self.per_pair || fast.pending()
    }

    /// Запросы следующего шага: одна первичная подписка или пара «отписка + подписка» потока spot.obu пары из очереди
    /// пересинхронизации; пусто — нечего. Ошибок нет.
    pub fn step(&mut self, fast: &mut Fast) -> Vec<String> {
        if self.next < self.assets.len() * self.per_pair {
            let i = self.next / self.per_pair;
            let is_obu = self.next % self.per_pair == 1;
            self.next += 1;
            return match fast.level() {
                Some(level) if is_obu => vec![self.obu_req("subscribe", i, level)],
                _ => vec![self.book_req("subscribe", i)],
            };
        }
        let (Some(s), Some(level)) = (fast.next_resync(), fast.level()) else {
            return Vec::new();
        };
        match self.assets.iter().position(|(_, x)| *x == s) {
            Some(i) => ["unsubscribe", "subscribe"]
                .map(|ev| self.obu_req(ev, i, level))
                .to_vec(),
            None => Vec::new(),
        }
    }

    /// Запрос spot.order_book пары. Принимает: событие (subscribe | unsubscribe), номер пары. Ошибок нет.
    pub fn book_req(&self, ev: &str, i: usize) -> String {
        let p = json!([self.assets[i].1, self.levels.to_string(), self.interval]);
        depth::request(BOOK_CHANNEL, ev, p, i as i64)
    }

    /// Запрос spot.obu пары. Принимает: событие, номер пары, уровни потока. Ошибок нет.
    fn obu_req(&self, ev: &str, i: usize, level: u64) -> String {
        let id = (self.assets.len() + i) as i64;
        obu::request(ev, &obu::stream(&self.assets[i].1, level), id)
    }
}
