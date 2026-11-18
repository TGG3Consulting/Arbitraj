//! Файл: core/manual_wait.rs
//! Что делает: решение 405 (t191, ответ Тиграна 18.11 на ревью Н3, вариант «в»): ручное действие с монетой, которую
//! держат идущие сделки, не отказывает, а ЖДЁТ их окончания. Кнопки, которые берут монету целиком
//! (coin_ride::take): «Продать сейчас» / «Продать и перегнать» остановленной сделки и «Продано вручную»
//! (manual_halted.rs), «Вернуть боту» (manual_return.rs), «Продать» на «Балансах» (balance_sell.rs). Порядок:
//! - нажатие — монета занята: если её держит застрявшая сделка (шаг «застряла»; закрытая, остановленная, «Беру на
//!   себя», «ждёт решения человека» — конец задачи сделки, ждём снятия записи — ревью 4 Б1, ревью 5 М2), окончания не
//!   будет — сразу отказ с причиной (какая сделка держит), паузы нет;
//! - иначе — пауза НОВЫХ сделок этой монеты (свой признак `inner.manual_waits`, не coin.blocked человека: входы
//!   исполнителя, очередь расчёта и take — coin_ride.rs), журнал core.manual_wait и экран «Ручное управление»: «ждёт
//!   окончания сделок по X: D1, D2»; ответ кнопке — «ждёт», не ошибка;
//! - задача ожидания (manual_wait_run.rs) просыпается при снятии записи идущей сделки, освобождении замка монеты и
//!   сохранении сделки (`inner.coin_wake`); все идущие кончились — та же команда человека (тот же источник и кто
//!   нажал) выполняется заново через Core::command — её итог, как у обычной команды; пауза держится, пока повтор не
//!   возьмёт монету (claim в coin_ride::take, ревью 4, Н3), не взял — снимается после команды;
//! - идущая сделка застряла — ожидание кончается отказом с причиной, пауза снимается; «Отменить ожидание» (кнопка
//!   экрана, команда CancelManualWait) — то же.
//!
//! Одна ожидающая команда на монету: вторая — отказ «уже ждёт …». Таймаута нет (настройки нет): ожидание кончается
//! окончанием сделок, уходом сделки с пути или кнопкой. Перезапуск (факт кода): ожидание — задача и запись в памяти
//! (команды человека не сохраняются, ManualCommand живёт в задаче Core::command) — после перезапуска их нет: пауза
//! снята, действие не выполнено, кнопку нужно нажать снова (идущие сделки после перезапуска — остановленные).
//! Команда не из Core::command (нет признака задачи) — прежний отказ.
//! Кто вызывает: manual_halted.rs, manual_return.rs, balance_sell.rs (defer), commands.rs (scope, command — отмена),
//! coin_ride.rs (paused), manual_return.rs (Core::manual — list).
//! От чего зависит: manual_wait_run.rs, coin_ride.rs, deal.rs, engine.rs, ids.rs, manual.rs (publish), deps.rs.
//! Настройки: нет.

use std::future::Future;
use std::sync::atomic::Ordering;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::core::types::{ManualAction, ManualCommand};
use crate::storage::journal::Level;

use super::coin_key::CoinKey;
use super::deal::Step;
use super::engine::{Core, lk};

tokio::task_local! {
    static CMD: ManualCommand;
    /// Решение 405 (ревью 5, М3): ID ожидания, повтор которого выполняется (None — обычная команда).
    static RETRY: Option<String>;
}

/// Ожидающее ручное действие монеты.
#[derive(Debug, Clone, Serialize)]
pub struct ManualWait {
    /// ID ожидания (для кнопки отмены).
    pub id: String,
    /// Монета (тикер).
    pub coin: String,
    /// Что ждёт (надпись кнопки).
    pub what: String,
    /// Кто нажал.
    pub who: String,
    /// Чьих окончаний ждёт (сделки; «вход сделки / ручное действие» — замок монеты целиком).
    pub waits_for: Vec<String>,
    /// С какого момента.
    pub since: DateTime<Utc>,
    /// Отменено кнопкой.
    #[serde(skip)]
    pub(super) cancel: bool,
    /// Команда, которую ждём (повтор узнаётся по ней — claim).
    #[serde(skip)]
    pub(super) action: ManualAction,
    /// Сделки кончились, повтор команды идёт: пауза держится, пока повтор не возьмёт монету (ревью 4, Н3).
    #[serde(skip)]
    pub(super) running: bool,
}

/// Что делает: выполнить команду человека с признаком задачи (для defer). Вызывает commands.rs. Ошибок нет.
pub async fn scope<F: Future>(cmd: ManualCommand, retry: Option<String>, f: F) -> F::Output {
    CMD.scope(cmd, RETRY.scope(retry, f)).await
}

pub use super::manual_wait_pause::{claim, is_retry, paused, paused_why, start_retry};

/// Что делает: действие текущей команды задачи (признак scope; для manual_wait_pause.rs is_retry). Ошибок нет.
pub(super) fn current_action() -> Option<ManualAction> {
    CMD.try_with(|c| c.action.clone()).ok()
}

/// Что делает: ID ожидания, повтор которого выполняет текущая задача (ревью 5, М3). Ошибок нет.
pub(super) fn current_retry() -> Option<String> {
    RETRY.try_with(Clone::clone).ok().flatten()
}

/// Сделка, держащая монету: ID, шаг, словами (deal_words.rs).
pub(super) type Holder = (String, Option<Step>, String);

/// Сделки монеты, которые держат её сейчас: (ID, шаг, словами) идущих и занят ли замок целиком. Ошибок нет.
pub(super) fn holders(core: &Core, key: &CoinKey) -> (Vec<Holder>, bool) {
    let rides = super::coin_ride::of_coin(core, key);
    let active = lk(&core.inner.active).contains(key);
    let deals = lk(&core.inner.deals);
    let v = rides
        .into_iter()
        .map(|r| match deals.get(&r.deal_id) {
            Some(d) => (r.deal_id, Some(d.step), super::deal_words::deal(core, d)),
            None => (r.deal_id.clone(), None, format!("сделка {}", r.deal_id)),
        })
        .collect();
    (v, active)
}

/// Сделка, которая держит монету и застряла (окончания не будет): текст причины. Ревью 4, Б1: закрыта / остановлена /
/// «Беру на себя» — это конец сделки (ledger::finish сохраняет шаг раньше, чем задача снимает запись в идущих —
/// coin_ride::release из ride_guard.rs; у группы ещё общий возврат), её запись вот-вот снимется — ждём. Ревью 5, М2:
/// «ждёт решения человека» — тоже конец задачи: шаг ставит только manual::raise_alert, и все его вызовы — последний шаг
/// задачи (stuck.rs, sell_unsent.rs, manual_sell.rs, relay_here.rs, remain_fact.rs all_foreign → вызывающий
/// возвращается) — запись снимется, ждём. Застревание — только «застряла» (режим застрявшей монеты ведёт её в этой же
/// задаче сколько угодно). Ошибок нет.
pub(super) fn stuck_of(list: &[Holder]) -> Option<String> {
    list.iter()
        .find(|(_, s, _)| matches!(s, Some(Step::Stuck)))
        .map(|(_, _, words)| super::manual_wait_text::stuck(words))
}

/// Список «чьих окончаний ждём» для экрана. Ошибок нет.
pub(super) fn waits_for(list: &[Holder], active: bool) -> Vec<String> {
    let mut v: Vec<String> = list.iter().map(|(_, _, words)| words.clone()).collect();
    if active {
        v.push(
            "вход новой сделки (предпроверка, покупка) или другое ручное действие с этой монетой"
                .into(),
        );
    }
    v
}

/// Что делает: монета занята — поставить команду человека в ожидание (см. шапку). Принимает: ядро, ключ монеты, тикер,
/// прежний текст отказа. Возвращает: Ok(«ждёт …») — ожидание начато; Err — прежний отказ (команда не из
/// Core::command), отказ с причиной (сделка вне пути, уже ждёт другая команда). Ошибок сверх этого нет.
pub fn defer(core: &Core, key: &CoinKey, ticker: &str, old: String) -> Result<String, String> {
    let Ok(cmd) = CMD.try_with(Clone::clone) else {
        return Err(old);
    };
    let (list, active) = holders(core, key);
    let what = cmd.action.title().to_string();
    if let Some(why) = stuck_of(&list) {
        return Err(super::manual_wait_text::refused(&what, ticker, &why));
    }
    let who = super::commands::who(&cmd);
    let w = ManualWait {
        id: super::ids::uuid_v4()
            .unwrap_or_else(|_| format!("mw{}", Utc::now().timestamp_micros())),
        coin: ticker.to_string(),
        what: what.clone(),
        who,
        waits_for: waits_for(&list, active),
        since: Utc::now(),
        cancel: false,
        action: cmd.action.clone(),
        running: false,
    };
    {
        let mut m = lk(&core.inner.manual_waits);
        if let Some(x) = m.get(key) {
            return Err(super::manual_wait_text::already(&what, ticker, x));
        }
        m.insert(key.clone(), w.clone());
        core.inner.manual_waits_live.fetch_add(1, Ordering::AcqRel);
    }
    let text = super::manual_wait_text::started(&w);
    super::manual_wait_run::start(core, key.clone(), w.id.clone(), cmd, text.clone());
    Ok(text)
}

/// Что делает: команда «Отменить ожидание» (CancelManualWait). Принимает: ядро, действие. Возвращает: None — действие
/// не отсюда; Some(итог). Ошибок сверх итога нет.
pub fn command(core: &Core, a: &ManualAction) -> Option<Result<String, String>> {
    let ManualAction::CancelManualWait { id } = a else {
        return None;
    };
    let found = {
        let mut m = lk(&core.inner.manual_waits);
        m.values_mut().find(|w| &w.id == id).map(|w| {
            w.cancel = true;
            super::manual_wait_text::cancelled(w)
        })
    };
    core.inner.coin_wake.notify_waiters();
    Some(match found {
        Some(t) => Ok(t),
        None => Err(format!("ожидания {id} нет (уже выполнено или отменено)")),
    })
}

/// Что делает: ожидания для экрана (GET /api/manual — waits). Ошибок нет.
pub fn list(core: &Core) -> Vec<ManualWait> {
    let mut v: Vec<ManualWait> = lk(&core.inner.manual_waits).values().cloned().collect();
    v.sort_by_key(|w| w.since);
    v
}

/// Что делает: снять ожидание (пауза новых сделок монеты снимается). Принимает: ядро, ключ, ID. Возвращает: снятое
/// (None — его уже нет или ID другой). Ошибок нет.
pub(super) fn remove(core: &Core, key: &CoinKey, id: &str) -> Option<ManualWait> {
    let mut m = lk(&core.inner.manual_waits);
    if m.get(key).is_some_and(|w| w.id == id) {
        core.inner.manual_waits_live.fetch_sub(1, Ordering::AcqRel);
        return m.remove(key);
    }
    None
}

/// Запись в журнал core.manual_wait. Ошибок нет.
pub(super) async fn log(core: &Core, level: Level, text: &str) {
    core.inner
        .deps
        .log(level, "core.manual_wait", None, text, None)
        .await;
}
