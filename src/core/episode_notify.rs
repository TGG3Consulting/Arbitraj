//! Файл: core/episode_notify.rs
//! Что делает: Telegram эпизодов (решение 74) с защитой от дребезга episode.min_notify_ms: открытие (deal.candidate с
//! карточкой сигнала и episode.opened) уходит, когда карточка готова и эпизод держится не меньше episode.min_notify_ms;
//! закрытие (episode.closed с итогом) — только если открытие было сообщено или эпизод продержался не меньше этого
//! (тогда сначала открытие — с карточкой или кратким текстом расчёта). Эпизод короче — в Telegram ничего, в файл,
//! базу и журнал — всё. episode.min_notify_ms не задана — сообщения сразу. Журнал карточки (core.candidate) — вместе с
//! открытием.
//! Кто вызывает: core/episode_worker.rs.
//! От чего зависит: card.rs, card_text.rs, costs.rs, episode.rs, episode_cfg.rs.
//! Настройки: episode.min_notify_ms (через Cfg).

use std::collections::HashMap;

use chrono::Utc;
use tokio::time::{Duration, Instant};

use crate::storage::journal::Level;

use super::card::Card;
use super::costs::Calc;
use super::engine::Core;
use super::episode::EpisodeInfo;
use super::episode_cfg::Cfg;

/// Открытый эпизод в задаче эпизодов.
pub struct Open {
    pub info: EpisodeInfo,
    /// Последний расчёт (краткий текст, если карточки нет).
    pub calc: Box<Calc>,
    /// Первая карточка эпизода — для сообщения об открытии.
    pub card: Option<Box<Card>>,
    /// Открытие уже сообщено в Telegram.
    pub notified: bool,
}

impl Open {
    /// Новый открытый эпизод. Ошибок нет.
    pub fn new(info: EpisodeInfo, calc: Box<Calc>) -> Self {
        Open {
            info,
            calc,
            card: None,
            notified: false,
        }
    }

    /// «CAKE bingx → bsc_dex». Ошибок нет.
    pub fn pair(&self) -> String {
        let d = &self.info.direction;
        format!("{} {} → {}", d.asset.ticker, d.buy_on.0, d.sell_on.0)
    }

    /// Сколько эпизод держится, мс. Ошибок нет.
    fn age_ms(&self) -> i64 {
        (self.info.ended_at.unwrap_or_else(Utc::now) - self.info.started_at).num_milliseconds()
    }
}

/// Порог дребезга, мс (не задан — 0). Ошибок нет.
fn min_ms(cfg: Option<&Cfg>) -> u64 {
    cfg.and_then(|c| c.min_notify_ms).unwrap_or(0)
}

/// Когда проснуться ради отложенного сообщения об открытии. Ошибок нет.
pub fn due(st: &HashMap<String, Open>, cfg: Option<&Cfg>) -> Option<Instant> {
    let min = min_ms(cfg) as i64;
    st.values()
        .filter(|o| !o.notified && o.card.is_some())
        .map(|o| Instant::now() + Duration::from_millis((min - o.age_ms()).max(0) as u64))
        .min()
}

/// Что делает: сообщает открытие, если пора (или force — перед закрытием, с кратким текстом без карточки).
/// Принимает: ядро, эпизод, настройки, force. Ошибок нет.
pub async fn opened(core: &Core, o: &mut Open, cfg: Option<&Cfg>, force: bool) {
    let ready = o.card.is_some() && o.age_ms() >= min_ms(cfg) as i64;
    if o.notified || !(ready || force) {
        return;
    }
    o.notified = true;
    let mark = mark(&o.info);
    let mut text = match &o.card {
        Some(c) => super::card_text::text(c),
        None => format!(
            "{mark}Сигнал: {}; объём {}, прибыль {} USDT ({}%)",
            o.pair(),
            o.calc.qty.normalize(),
            o.calc.profit_usd.round_dp(4),
            o.calc.profit_pct.round_dp(4)
        ),
    };
    if let Some(s) = o.calc.size.as_ref() {
        text.push_str(&format!("\nОбъём: {}", s.why));
    }
    let deps = &core.inner.deps;
    let venue = o.info.direction.buy_on.0.clone();
    deps.log(Level::Info, "core.candidate", Some(&venue), &text, None)
        .await;
    deps.notify("deal.candidate", Some(&venue), &text).await;
    let short = format!(
        "{mark}Эпизод открыт: {}; прибыль {} USDT; id {}",
        o.pair(),
        o.info.max_profit.round_dp(4),
        o.info.id
    );
    deps.notify("episode.opened", Some(&venue), &short).await;
}

/// Что делает: сообщение о закрытии по правилам дребезга (см. шапку). Принимает: ядро, закрытый эпизод (info с
/// ended_at), настройки. Ошибок нет.
pub async fn closed(core: &Core, o: &mut Open, cfg: Option<&Cfg>) {
    if !o.notified && o.age_ms() < min_ms(cfg) as i64 {
        return;
    }
    opened(core, o, cfg, true).await;
    let text = closed_text(&o.info);
    core.inner
        .deps
        .notify("episode.closed", Some(&o.info.direction.buy_on.0), &text)
        .await;
}

/// «[СУХОЙ] » у сухого эпизода. Ошибок нет.
pub fn mark(i: &EpisodeInfo) -> &'static str {
    if i.dry { "[СУХОЙ] " } else { "" }
}

/// Итог закрытия текстом. Ошибок нет.
pub fn closed_text(i: &EpisodeInfo) -> String {
    let d = &i.direction;
    let secs = i
        .ended_at
        .map(|e| (e - i.started_at).num_milliseconds() as f64 / 1000.0)
        .unwrap_or_default();
    format!(
        "{}Эпизод закрыт: {} {} → {}; длительность {secs:.1} с; макс. прибыль {} USDT{}; записей {}; причина: {}",
        mark(i),
        d.asset.ticker,
        d.buy_on.0,
        d.sell_on.0,
        i.max_profit.round_dp(4),
        i.max_profit_unbounded
            .map(|v| format!(" (без бюджета {})", v.round_dp(4)))
            .unwrap_or_default(),
        i.records,
        i.close_reason.as_deref().unwrap_or("")
    )
}
