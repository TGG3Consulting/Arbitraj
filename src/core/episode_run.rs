//! Файл: core/episode_run.rs
//! Что делает: вход в эпизоды арбитражной ситуации (решение 74) для очереди кандидатов и тика — только память и
//! отправка сообщений, без диска, базы и сети (их делает задача episode_worker.rs): прошедший кандидат открывает эпизод
//! или даёт запись (не чаще episode.min_interval_ms — решает episode.rs); отказ, выход сигнала из «кандидата» или
//! пропажа направления из справочника (sweep — при пересборке и по таймеру) закрывают эпизод с причиной.
//! Торговый путь эпизод не трогает: пересчёт кандидатов идёт как раньше, эпизод только наблюдает.
//! Кто вызывает: core/candidates.rs (on_accept, failed), core/evaluate.rs (failed), core/rebuild.rs и
//! episode_worker.rs (sweep), core/card_job.rs (send).
//! От чего зависит: episode.rs, episode_cfg.rs (Cfg), episode_snap.rs, inner.rs (каналы и память).
//! Настройки: episode.* (episode_cfg.rs).

use std::collections::HashSet;

use chrono::Utc;

use crate::core::types::Direction;

use super::card::Card;
use super::costs::Calc;
use super::engine::{Core, lk, rd};
use super::episode::{EpisodeInfo, Step};
use super::episode_cfg::Cfg;
use super::episode_snap::{Snap, capture};

/// Сообщение задаче эпизодов (episode_worker.rs). Порядок сообщений — порядок событий.
#[derive(Debug)]
pub enum EpMsg {
    /// Эпизод открыт: заголовок файла, сводка, журнал; уведомление — когда будет карточка (и после episode.min_notify_ms).
    Open { info: EpisodeInfo, calc: Box<Calc> },
    /// Запись пересчёта (снимок — на момент пересчёта, карточка — построена после).
    Record {
        info: EpisodeInfo,
        calc: Box<Calc>,
        snap: Box<Snap>,
        card: Option<Box<Card>>,
    },
    /// Эпизод закрыт.
    Close { info: EpisodeInfo },
    /// Эпизоды перестали вестись (или снова ведутся — None): одна запись в журнал на смену.
    Off(Option<String>),
}

/// Что делать с прошедшим кандидатом.
#[derive(Debug)]
pub enum Accepted {
    /// Эпизоды не ведутся (не заданы episode.*).
    Off,
    /// Эпизод открыт, запись пропущена (чаще episode.min_interval_ms).
    Skip,
    /// Нужна запись: эпизод, снимок рынка, первая ли (эпизод открыт сейчас).
    Ep {
        info: Box<EpisodeInfo>,
        snap: Box<Snap>,
        first: bool,
    },
}

/// Что делает: кандидат прошёл — открыть эпизод или подготовить запись. Только память и сообщение Open.
/// Принимает: ядро, расчёт, сухой ли режим. Возвращает: Accepted. Ошибок нет.
pub fn on_accept(core: &Core, calc: &Calc, dry: bool) -> Accepted {
    let Some(cfg) = current(core) else {
        return Accepted::Off;
    };
    let d = &calc.direction;
    let now = Utc::now();
    let step = lk(&core.inner.episodes).accept(
        d,
        new_id(d, now),
        dry,
        calc.profit_usd,
        None,
        now,
        cfg.min_interval_ms,
    );
    let (info, first) = match step {
        Step::Skip => return Accepted::Skip,
        Step::Record(info) => (info, false),
        Step::Opened(info) => {
            send(
                core,
                EpMsg::Open {
                    info: info.clone(),
                    calc: Box::new(calc.clone()),
                },
            );
            (info, true)
        }
    };
    let snap = Box::new(capture(core, calc, cfg.book_levels));
    Accepted::Ep {
        info: Box::new(info),
        snap,
        first,
    }
}

/// Что делает: направление перестало проходить — закрыть открытый эпизод с причиной (сообщение Close) и забыть, что
/// сухой сигнал уже сообщён. Принимает: ядро, направление, причину. Ошибок нет (эпизода нет — ничего).
pub fn failed(core: &Core, d: &Direction, reason: &str) {
    lk(&core.inner.dry_told).remove(d);
    if let Some(info) = lk(&core.inner.episodes).close(d, reason, Utc::now()) {
        send(core, EpMsg::Close { info });
    }
}

/// Что делает: закрывает эпизоды направлений, которых больше нет в справочнике (монета ушла, направление исчезло), —
/// «направление больше не отслеживается»; так же чистит отметки сухих сигналов. Ошибок нет.
pub fn sweep(core: &Core) {
    let live: HashSet<Direction> = rd(&core.inner.coins)
        .iter()
        .flat_map(|c| c.shown_directions())
        .collect();
    lk(&core.inner.dry_told).retain(|d| live.contains(d));
    let gone: Vec<Direction> = lk(&core.inner.episodes)
        .list()
        .into_iter()
        .map(|e| e.direction)
        .filter(|d| !live.contains(d))
        .collect();
    for d in gone {
        failed(core, &d, "направление больше не отслеживается");
    }
}

/// Открыт ли эпизод направления. Ошибок нет.
pub fn is_open(core: &Core, d: &Direction) -> bool {
    lk(&core.inner.episodes).is_open(d)
}

/// Сообщение задаче эпизодов без ожидания (try_send). Канал полон — сообщение теряется: счётчик и запись в журнал на
/// 1-ю, 2-ю, 4-ю, 8-ю… потерю (не чаще); потерянное закрытие уборка задачи доводит сама (episode_cfg.rs). Ошибок нет.
pub fn send(core: &Core, m: EpMsg) {
    use std::sync::atomic::Ordering;
    use tokio::sync::mpsc::error::TrySendError;
    let kind = match &m {
        EpMsg::Open { .. } => "открытие",
        EpMsg::Record { .. } => "запись",
        EpMsg::Close { .. } => "закрытие",
        EpMsg::Off(_) => "смена «ведутся / нет»",
    };
    if let Err(TrySendError::Full(_)) = core.inner.episode_tx.try_send(m) {
        let n = core.inner.episode_lost.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            let c = core.clone();
            tokio::spawn(async move {
                let text = format!(
                    "очередь эпизодов полна: потеряно сообщений — {n} (последнее: {kind}); записи эпизодов неполные"
                );
                super::episode_cfg::warn(&c, text).await;
            });
        }
    }
}

/// Настройки эпизодов; смена «ведутся / нет» — сообщение Off (журнал пишет задача). Ошибок нет.
fn current(core: &Core) -> Option<Cfg> {
    let r = Cfg::read(&core.inner.deps.settings);
    let why = r.as_ref().err().cloned();
    let changed = {
        let mut off = lk(&core.inner.episode_off);
        let c = *off != why;
        *off = why.clone();
        c
    };
    if changed {
        send(core, EpMsg::Off(why));
    }
    r.ok()
}

/// id эпизода: время до мс, тикер, площадки — только буквы, цифры и «-» (годится как имя файла). Ошибок нет.
fn new_id(d: &Direction, now: chrono::DateTime<Utc>) -> String {
    let clean = |s: &str| -> String { s.chars().filter(char::is_ascii_alphanumeric).collect() };
    format!(
        "{}-{}-{}-{}",
        now.format("%Y%m%d%H%M%S%3f"),
        clean(&d.asset.ticker),
        clean(&d.buy_on.0),
        clean(&d.sell_on.0)
    )
}
