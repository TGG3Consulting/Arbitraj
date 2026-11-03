//! Файл: core/window_run.rs
//! Что делает: вход в окна (решение 125) для очереди кандидатов и тика — только память и сообщение задаче окон
//! (window_worker.rs), без диска, сети и ожидания: прошедший расчёт кандидат открывает окно монеты или входит в
//! открытое (window.rs); отказ расчёта, выход сигнала из «кандидата» или пропажа направления из справочника — направление
//! больше не проходит (толчок задаче: пора ли закрывать). Окно пишется только на биржах направления (CEX ↔ CEX — обе,
//! CEX ↔ DEX — одна); у DEX стакана и ленты сделок нет.
//! Торговый путь окно не трогает: оно только наблюдает.
//! Кто вызывает: core/candidates.rs (pass, fail), core/episode_run.rs (fail, sweep — общий путь «перестало
//! проходить»), core/evaluate.rs (passing), core/card_job.rs (unbounded).
//! От чего зависит: window.rs, window_cfg.rs (Cfg), inner.rs (каналы и память).
//! Настройки: window.* (window_cfg.rs).

use std::collections::HashSet;

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Direction, VenueId};
use crate::gateway::contract::VenueKind;

use super::costs::Calc;
use super::engine::{Core, lk, rd};
use super::window::{Pass, WindowInfo};
use super::window_cfg::Cfg;

/// Сообщение задаче окон. Порядок сообщений — порядок событий.
#[derive(Debug)]
pub enum WinMsg {
    /// Окно открыто: файл, приёмник, запись на биржах, сводка, журнал.
    Open { info: Box<WindowInfo> },
    /// В открытое окно вошло новое направление (может быть — с новой биржей).
    Join { info: Box<WindowInfo> },
    /// Окна перестали писаться (или снова пишутся — None): одна запись в журнал на смену.
    Off(Option<String>),
}

/// Что делает: кандидат прошёл расчёт прибыльности — открыть окно монеты или войти в открытое. Только память и
/// сообщение. Принимает: ядро, расчёт, сухой ли режим. Ошибок нет (окна не пишутся — ничего).
pub fn pass(core: &Core, calc: &Calc, dry: bool) {
    if current(core).is_none() {
        return;
    }
    let d = &calc.direction;
    let cex = cex_of(core, d);
    if cex.is_empty() {
        return;
    }
    let now = Utc::now();
    let step = lk(&core.inner.windows).pass(d, new_id(d, now), dry, calc.profit_usd, &cex, now);
    match step {
        Pass::Opened(info) => send(
            core,
            WinMsg::Open {
                info: Box::new(info),
            },
        ),
        Pass::Joined(info) => send(
            core,
            WinMsg::Join {
                info: Box::new(info),
            },
        ),
        Pass::Same => {}
    }
}

/// Что делает: направление перестало проходить — убрать из проходящих; не проходит ни одно — толчок задаче окон
/// (срок window.min_hold_ms). Принимает: ядро, направление. Ошибок нет.
pub fn fail(core: &Core, d: &Direction) {
    if lk(&core.inner.windows).fail(d, Utc::now()) {
        core.inner.window_kick.notify_one();
    }
}

/// Проходит ли направление в открытом окне (тик: сигнал вышел из «кандидата» — надо сообщить fail). Ошибок нет.
pub fn passing(core: &Core, d: &Direction) -> bool {
    lk(&core.inner.windows).passing(d)
}

/// Наибольшая прибыль без бюджета из карточки сигнала. Принимает: ядро, направление, прибыль. Ошибок нет.
pub fn unbounded(core: &Core, d: &Direction, v: Option<Decimal>) {
    if let Some(v) = v {
        lk(&core.inner.windows).note_unbounded(d, v);
    }
}

/// Что делает: направления окон, которых больше нет в справочнике, перестали проходить. Принимает: ядро, живые
/// направления. Ошибок нет.
pub fn sweep(core: &Core, live: &HashSet<Direction>) {
    let gone: Vec<Direction> = lk(&core.inner.windows)
        .all_passing()
        .into_iter()
        .filter(|d| !live.contains(d))
        .collect();
    for d in gone {
        fail(core, &d);
    }
}

/// Биржи (CEX) направления — на них пишется окно. Ошибок нет.
fn cex_of(core: &Core, d: &Direction) -> Vec<VenueId> {
    let kinds = rd(&core.inner.kinds);
    [&d.buy_on, &d.sell_on]
        .into_iter()
        .filter(|v| kinds.iter().any(|(id, k)| id == *v && *k == VenueKind::Cex))
        .cloned()
        .collect()
}

/// Сообщение задаче окон без ожидания (try_send). Канал полон — сообщение теряется: счётчик и запись в журнал на
/// 1-ю, 2-ю, 4-ю… потерю. Ошибок нет.
pub fn send(core: &Core, m: WinMsg) {
    use std::sync::atomic::Ordering;
    use tokio::sync::mpsc::error::TrySendError;
    if let Err(TrySendError::Full(_)) = core.inner.window_tx.try_send(m) {
        let n = core.inner.window_lost.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            let c = core.clone();
            tokio::spawn(async move {
                let text =
                    format!("очередь окон полна: потеряно сообщений — {n}; окно может не писаться");
                super::window_cfg::warn(&c, text).await;
            });
        }
    }
}

/// Настройки окон; смена «пишутся / нет» — сообщение Off (журнал пишет задача). Ошибок нет.
pub fn current(core: &Core) -> Option<Cfg> {
    let r = Cfg::read(&core.inner.deps.settings);
    let why = r.as_ref().err().cloned();
    let changed = {
        let mut off = lk(&core.inner.window_off);
        let c = *off != why;
        *off = why.clone();
        c
    };
    if changed {
        send(core, WinMsg::Off(why));
    }
    r.ok()
}

/// id окна: время до мс и тикер — только буквы, цифры и «-» (годится как имя файла). Ошибок нет.
fn new_id(d: &Direction, now: chrono::DateTime<Utc>) -> String {
    let t: String = d
        .asset
        .ticker
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    format!("{}-{t}", now.format("%Y%m%d%H%M%S%3f"))
}
