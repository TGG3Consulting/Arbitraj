//! Файл: venues/mexc/fast_pair.rs
//! Что делает: решение 420 (Тигран 20.11) — быстрые стаканы пар одного соединения MEXC и страховка потоком снимков.
//! Соединение слушает оба канала пары: изменения spot@public.aggre.depth.v3.api.pb (быстрый стакан — fast_book.rs,
//! снимок REST из очереди fast_snap.rs) и частичный стакан spot@public.limit.depth.v3.api.pb (снимки, как раньше).
//! Ядру — быстрый стакан (верхние venue.depth_levels уровней), пока он синхронизирован; разрыв версий, неразобранный
//! кадр изменений, отказ подписки, переподписка сторожем — сразу на снимки: последний кадр потока снимков ядру (если он
//! не старее отданного — ws_out.rs), быстрый стакан — снимок REST заново по официальной процедуре; синхронизирован и
//! пришёл кадр изменений — снова быстрый. Каждое переключение и пересинхронизация — счётчик и строка журнала
//! (fast_stats.rs).
//! Сверка с потоком снимков — только по содержимому, без сопоставления номеров: у кадра частичного стакана в
//! документации поле version — «Version number», связи с fromVersion/toVersion канала изменений не описано; время
//! sendTime обоих — «Event time», но кадр изменений собирается за интервал, и одно и то же состояние книги по времени не
//! сопоставить (решение 420, открытый вопрос — docs/TASKS.md). Поэтому: верхние уровни кадра снимков равны быстрому
//! стакану — быстрый подтверждён (время кадра пары продлевается, book.rs touch); не равны — ничего, пока быстрый поток
//! жив; быстрый стакан признаётся отставшим (на снимки и снимок REST заново), если (ревью 420) два кадра снимков подряд
//! новее его по sendTime и не равны ему, а кадров изменений пары между ними не было, или если не равны и кадров
//! изменений пары (и подтверждений) нет дольше venue.book_stale_ms.
//! Снимки REST, их итоги и переключение на снимки — fast_resync.rs.
//! Кто вызывает: ws_sess.rs.
//! От чего зависит: fast_book.rs, fast_resync.rs, fast_stats.rs, ws_out.rs, ctx.rs.
//! Настройки: venue.book_stale_ms [mexc].

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::core::types::{Asset, PriceLevel};

use super::ctx::Ctx;
use super::fast_book::{Applied, LocalBook, Update};
use super::fast_snap::Note;
use super::fast_stats::{self, Ev};
use super::ws_out::{Gave, Out};

/// Состояние пары.
#[derive(Default)]
pub(super) struct Pair {
    pub(super) book: LocalBook,
    /// Ядру сейчас отдаётся быстрый стакан.
    pub(super) fast: bool,
    /// Канал изменений пары отклонён биржей — до конца сессии только снимки.
    pub(super) off: bool,
    /// Снимок REST в очереди или в пути.
    pub(super) pending: bool,
    /// Сколько снимков REST запрошено (первый — синхронизация, остальные — пересинхронизации).
    pub(super) asked: u64,
    /// Повтор снимка после сбоя не раньше.
    pub(super) retry_at: Option<Instant>,
    /// Последнее подтверждение быстрого стакана: кадр изменений или равный кадр снимков.
    pub(super) active: Option<Instant>,
    /// Ревью 420: прошлый кадр снимков не совпал с быстрым стаканом и был новее его (sendTime), кадров изменений с тех пор нет.
    pub(super) miss: bool,
    /// Последний кадр потока снимков: уровни, время биржи, время получения (unix мс, часы бота).
    pub(super) snap: Option<Snapped>,
}

/// Кадр потока снимков: уровни, время биржи, время получения (unix мс).
pub(super) type Snapped = (Vec<PriceLevel>, Vec<PriceLevel>, Option<DateTime<Utc>>, i64);

/// Быстрые стаканы соединения.
pub struct Fast {
    pub(super) pairs: HashMap<String, Pair>,
    pub(super) levels: usize,
    pub(super) tx: mpsc::Sender<Note>,
}

/// Время sendTime (unix мс) → время ядра. Ошибок нет.
pub(super) fn to_time(ms: Option<i64>) -> Option<DateTime<Utc>> {
    ms.and_then(|ms| Utc.timestamp_millis_opt(ms).single())
}

impl Fast {
    /// Что делает: состояние сессии. Принимает: уровней ядру (venue.depth_levels), канал итогов очереди снимков.
    /// Ошибок нет.
    pub fn new(levels: u64, tx: mpsc::Sender<Note>) -> Self {
        Self {
            pairs: HashMap::new(),
            levels: levels as usize,
            tx,
        }
    }

    /// Что делает: кадр потока снимков пары (правила — шапка). Принимает: окружение, отдачу, монету, символ, уровни,
    /// время биржи. Возвращает: итог отдачи; None — ядру ничего не отдавалось. Ошибок нет.
    #[allow(clippy::too_many_arguments)]
    pub fn on_limit(
        &mut self,
        ctx: &Ctx,
        out: &mut Out<'_>,
        asset: &Asset,
        symbol: &str,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        time: Option<DateTime<Utc>>,
    ) -> Option<Gave> {
        let levels = self.levels;
        let p = self.pairs.entry(symbol.to_string()).or_default();
        p.snap = Some((
            bids.clone(),
            asks.clone(),
            time,
            Utc::now().timestamp_millis(),
        ));
        if !p.fast {
            p.miss = false;
            return Some(out.give(asset, symbol, bids, asks, time));
        }
        let (fb, fa) = p.book.top(levels);
        if fb == bids && fa == asks {
            p.active = Some(Instant::now());
            p.miss = false;
            return Some(out.give(asset, symbol, fb, fa, to_time(p.book.time_ms())));
        }
        // Ревью 420: кадр снимков новее быстрого стакана (sendTime) и не совпал — второй такой подряд без кадров
        // изменений между ними: быстрый отстал (канал изменений — «(100ms|10ms)», Diff.Depth Stream; кадр снимков — раз
        // в ~500 мс, замер решения 420; живой проверки правила нет — TASKS).
        let newer = matches!((time, to_time(p.book.time_ms())), (Some(ts), Some(tf)) if ts > tf);
        let lag = newer && p.miss;
        p.miss = newer;
        let quiet = p.active.map(|a| a.elapsed()).unwrap_or(Duration::MAX);
        let limit = ctx.int("venue.book_stale_ms").ok();
        let why = if lag {
            "два кадра снимков подряд новее быстрого стакана (sendTime) и не равны ему, кадров изменений между ними нет"
                .to_string()
        } else if let Some(l) = limit.filter(|l| quiet > Duration::from_millis(*l)) {
            format!(
                "кадров изменений и подтверждений нет {} мс (venue.book_stale_ms = {l}), а поток снимков показывает другую книгу",
                quiet.as_millis()
            )
        } else {
            return None;
        };
        p.miss = false;
        p.book.reset(why.clone());
        self.fallback(ctx, out, asset, symbol, &why)
    }

    /// Что делает: кадр изменений пары. Принимает: окружение, отдачу, монету, символ, кадр или причину, почему он не
    /// разобран. Возвращает: итог отдачи; None — ядру ничего не отдавалось. Ошибок нет.
    pub fn on_diff(
        &mut self,
        ctx: &Ctx,
        out: &mut Out<'_>,
        asset: &Asset,
        symbol: &str,
        frame: Result<Update, String>,
    ) -> Option<Gave> {
        let p = self.pairs.entry(symbol.to_string()).or_default();
        if p.off {
            return None;
        }
        p.miss = false;
        let applied = match frame {
            Ok(u) => p.book.apply(u),
            Err(why) => p.book.reset(format!("кадр изменений не разобран: {why}")),
        };
        self.after(ctx, out, asset, symbol, applied)
    }

    /// Что делает: переподписка пары сторожем или отказ подписки канала изменений (off) — на снимки, быстрый стакан
    /// заново. Принимает: окружение, отдачу, монету, символ, отключить ли канал изменений до конца сессии, причину.
    /// Возвращает: итог отдачи последнего кадра снимков. Ошибок нет.
    pub fn drop_pair(
        &mut self,
        ctx: &Ctx,
        out: &mut Out<'_>,
        (asset, symbol): (&Asset, &str),
        off: bool,
        why: &str,
    ) -> Option<Gave> {
        let p = self.pairs.entry(symbol.to_string()).or_default();
        p.off |= off;
        p.book.reset(why.to_string());
        self.fallback(ctx, out, asset, symbol, why)
    }

    /// Что делать после кадра изменений или снимка REST. Возвращает: итог отдачи. Ошибок нет.
    pub(super) fn after(
        &mut self,
        ctx: &Ctx,
        out: &mut Out<'_>,
        asset: &Asset,
        symbol: &str,
        applied: Applied,
    ) -> Option<Gave> {
        let levels = self.levels;
        let p = self.pairs.entry(symbol.to_string()).or_default();
        match applied {
            // Снимок без кадров после него — времени биржи у стакана нет: ядру — с первым кадром изменений.
            Applied::Changed if p.book.time_ms().is_none() => None,
            Applied::Changed => {
                p.active = Some(Instant::now());
                if !p.fast {
                    p.fast = true;
                    fast_stats::note(&ctx.journal, Ev::ToFast, &asset.ticker, "синхронизирован");
                }
                let (b, a) = p.book.top(levels);
                Some(out.give(asset, symbol, b, a, to_time(p.book.time_ms())))
            }
            Applied::Repeat => {
                p.active = Some(Instant::now());
                None
            }
            Applied::Waiting | Applied::Older => {
                self.ask(ctx, asset, symbol);
                None
            }
            Applied::Gap(why) => self.fallback(ctx, out, asset, symbol, &why),
        }
    }
}
