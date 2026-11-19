//! Файл: venues/binance/sbe_pair.rs
//! Что делает: решение 425 — пары одного соединения SBE Binance: кадр изменений → локальный стакан пары (sbe_book.rs),
//! снимок REST просится из общей очереди (sbe_snap.rs) один раз на пару; пропуск номеров или сбой снимка — пара сразу
//! на страховку JSON (fast.rs), новый снимок не раньше venue.ws_reconnect_pause_ms (без круга ресинков, урок 12);
//! пункт 4 процедуры (lastUpdateId < U) — снимок заново сразу. Изменился стакан — верхние venue.depth_levels уровней
//! ядру через fast.rs (формат ядра тот же, что у JSON).
//! Кто вызывает: sbe_conn.rs (кадры, снимки, команды пересинхронизации из fast.rs / fast_check.rs).
//! От чего зависит: sbe.rs (Diff), sbe_book.rs, sbe_snap.rs, fast.rs, depth.rs (to_book), book.rs (Alive), ctx.rs.
//! Настройки: venue.ws_reconnect_pause_ms [binance] (пауза перед новым снимком после пропуска); venue.depth_levels
//! (уровней ядру — из ws.rs Cfg).
//!
//! Официально: web-socket-streams.md «How to manage a local order book correctly» (через sbe_book.rs);
//! sbe-market-data-streams.md: «All time and timestamp fields are in microseconds» — eventTime → время биржи стакана.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::core::types::Asset;
use crate::gateway::core_out_book::Fed;

use super::book::Alive;
use super::ctx::Ctx;
use super::depth;
use super::fast::Fast;
use super::sbe::Diff;
use super::sbe_book::{Applied, LocalBook, Update};
use super::sbe_snap::{self, Done};

/// Пары соединения SBE.
pub struct Pairs<'a> {
    pub ctx: &'a Ctx,
    pub fast: &'a Fast,
    /// Символ строчными → (монета, символ Binance).
    pub by_key: HashMap<String, (&'a Asset, &'a str)>,
    pub levels: usize,
    pub resync_pause: Duration,
    pub snap_tx: mpsc::Sender<Done>,
    pub alive: Alive,
    local: HashMap<String, LocalBook>,
    pending: HashSet<String>,
    not_before: HashMap<String, Instant>,
}

impl<'a> Pairs<'a> {
    /// Что делает: пары соединения. Принимает: окружение, арбитр, монеты (монета, символ), уровней ядру, паузу
    /// после пропуска, канал снимков, живость соединения. Ошибок нет.
    pub fn new(
        ctx: &'a Ctx,
        fast: &'a Fast,
        assets: &'a [(Asset, String)],
        levels: usize,
        resync_pause: Duration,
        snap_tx: mpsc::Sender<Done>,
        alive: Alive,
    ) -> Self {
        let by_key = assets
            .iter()
            .map(|(a, s)| (s.to_lowercase(), (a, s.as_str())))
            .collect();
        Pairs {
            ctx,
            fast,
            by_key,
            levels,
            resync_pause,
            snap_tx,
            alive,
            local: HashMap::new(),
            pending: HashSet::new(),
            not_before: HashMap::new(),
        }
    }

    /// Что делает: кадр изменений. Принимает: разобранный кадр. Возвращает: Ok(false) — канал ядра закрыт.
    /// Ошибки: текст — символа нет среди пар соединения.
    pub fn on_diff(&mut self, d: Diff) -> Result<bool, String> {
        let key = d.symbol.to_lowercase();
        if !self.by_key.contains_key(&key) {
            return Err(format!(
                "кадр SBE по символу «{}», которого нет в соединении",
                d.symbol
            ));
        }
        let u = Update {
            first: d.first,
            last: d.last,
            bids: d.bids,
            asks: d.asks,
        };
        let applied = self.local.entry(key.clone()).or_default().apply(u);
        Ok(self.after(&key, applied, Some(d.event_us)))
    }

    /// Что делает: сообщение очереди снимков. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub fn on_done(&mut self, done: Done) -> bool {
        match done {
            Done::Start(key) => {
                self.local.entry(key).or_default().begin();
                true
            }
            Done::Snap(key, res) => {
                self.pending.remove(&key);
                let lb = self.local.entry(key.clone()).or_default();
                let applied = match res {
                    Ok(s) => lb.snapshot(s.id, &s.bids, &s.asks),
                    Err(why) => lb.reset(format!("снимок REST не получен: {why}")),
                };
                self.after(&key, applied, None)
            }
        }
    }

    /// Что делает: пересинхронизация пары — быстрый стакан заново со снимка. Принимает: тикер, после паузы ли:
    /// false — сторож ядра переподписал пару (снимок сразу); true — быстрый разошёлся со страховкой JSON (ревью 425,
    /// fast_check.rs): снимок не раньше venue.ws_reconnect_pause_ms, как после пропуска (без круга ресинков). Ошибок нет.
    pub fn resync(&mut self, ticker: &str, paused: bool) {
        let Some(key) = self
            .by_key
            .iter()
            .find(|(_, (a, _))| a.ticker == ticker)
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        self.local
            .entry(key.clone())
            .or_default()
            .reset(String::new());
        let why = if paused {
            "расхождение со страховкой JSON — снимок заново после паузы"
        } else {
            "сторож ядра переподписал пару — снимок заново"
        };
        if let Some((a, _)) = self.by_key.get(&key) {
            self.fast.fast_lost(a, why, true);
        }
        if paused {
            self.not_before
                .insert(key, Instant::now() + self.resync_pause);
        } else {
            self.not_before.remove(&key);
            self.ask(&key);
        }
    }

    /// Что делать после изменения локального стакана. Принимает: ключ, итог, время события (мкс). Возвращает: false —
    /// канал ядра закрыт. Ошибок нет.
    fn after(&mut self, key: &str, a: Applied, event_us: Option<i64>) -> bool {
        let Some(&(asset, _)) = self.by_key.get(key) else {
            return true;
        };
        match a {
            Applied::Changed => {
                let (bids, asks) = self
                    .local
                    .get(key)
                    .map(|l| l.top(self.levels))
                    .unwrap_or_default();
                let mut ob = depth::to_book(asset.clone(), bids, asks);
                ob.exchange_time = event_us.and_then(chrono::DateTime::from_timestamp_micros);
                self.fast.fast_book(ob, &self.alive) != Fed::Closed
            }
            Applied::Old | Applied::Buffered => true,
            Applied::Idle => {
                self.ask(key);
                true
            }
            Applied::Retry => {
                self.fast.fast_lost(
                    asset,
                    "снимок старше первого изменения (пункт 4) — снимок заново",
                    true,
                );
                self.ask(key);
                true
            }
            Applied::Gap(why) => {
                self.fast.fast_lost(asset, &why, true);
                self.not_before
                    .insert(key.to_string(), Instant::now() + self.resync_pause);
                true
            }
        }
    }

    /// Просит снимок пары, если он ещё не просится и пауза после пропуска прошла. Ошибок нет.
    fn ask(&mut self, key: &str) {
        let allowed = self
            .not_before
            .get(key)
            .is_none_or(|at| Instant::now() >= *at);
        if !allowed || self.pending.contains(key) {
            return;
        }
        if let Some(&(_, symbol)) = self.by_key.get(key) {
            self.pending.insert(key.to_string());
            sbe_snap::request(
                self.ctx,
                key.to_string(),
                symbol.to_string(),
                self.snap_tx.clone(),
            );
        }
    }
}
