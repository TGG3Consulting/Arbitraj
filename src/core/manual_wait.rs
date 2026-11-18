//! Файл: core/manual_wait.rs
//! Что делает: решение 405 (t191, ответ Тиграна 18.11 на ревью Н3, вариант «в»): ручное действие с монетой, которую
//! держат идущие сделки, не отказывает, а ЖДЁТ их окончания. Кнопки, которые берут монету целиком
//! (coin_ride::take): «Продать сейчас» / «Продать и перегнать» остановленной сделки и «Продано вручную»
//! (manual_halted.rs), «Вернуть боту» (manual_return.rs), «Продать» на «Балансах» (balance_sell.rs). Порядок:
//! - нажатие — монета занята: если её держит сделка вне пути (застряла, ждёт человека — шаг не «перевод» и не
//!   «продажа»), окончания не будет — сразу отказ с причиной (какая сделка держит), паузы нет;
//! - иначе — пауза НОВЫХ сделок этой монеты (свой признак `inner.manual_waits`, не coin.blocked человека: входы
//!   исполнителя, очередь расчёта и take — coin_ride.rs), журнал core.manual_wait и экран «Ручное управление»: «ждёт
//!   окончания сделок по X: D1, D2»; ответ кнопке — «ждёт», не ошибка;
//! - задача ожидания (manual_wait_run.rs) просыпается при снятии записи идущей сделки, освобождении замка монеты и
//!   сохранении сделки (`inner.coin_wake`); все идущие кончились — пауза снимается и та же команда человека (тот же
//!   источник и кто нажал) выполняется заново через Core::command — её итог, как у обычной команды;
//! - идущая сделка ушла с пути — ожидание кончается отказом с причиной, пауза снимается; «Отменить ожидание» (кнопка
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
}

/// Что делает: выполнить команду человека с признаком задачи (для defer). Вызывает commands.rs. Ошибок нет.
pub async fn scope<F: Future>(cmd: ManualCommand, f: F) -> F::Output {
    CMD.scope(cmd, f).await
}

/// Что делает: стоит ли пауза новых сделок монеты (ждёт ручное действие). Быстрый путь — атомарный счётчик.
/// Принимает: ядро, ключ монеты. Ошибок нет.
pub fn paused(core: &Core, key: &CoinKey) -> bool {
    core.inner.manual_waits_live.load(Ordering::Acquire) != 0
        && lk(&core.inner.manual_waits).contains_key(key)
}

/// Что делает: причина отказа входа сделки при паузе. Принимает: ядро, ключ. Возвращает: текст; None — паузы нет.
/// Ошибок нет.
pub fn paused_why(core: &Core, key: &CoinKey) -> Option<String> {
    if core.inner.manual_waits_live.load(Ordering::Acquire) == 0 {
        return None;
    }
    lk(&core.inner.manual_waits).get(key).map(|w| {
        format!(
            "новые сделки {} на паузе: ручное действие «{}» ждёт окончания сделок (решение 405)",
            w.coin, w.what
        )
    })
}

/// Сделки монеты, которые держат её сейчас: (ID, шаг) идущих и занят ли замок целиком. Ошибок нет.
pub(super) fn holders(core: &Core, key: &CoinKey) -> (Vec<(String, Option<Step>)>, bool) {
    let rides = super::coin_ride::of_coin(core, key);
    let active = lk(&core.inner.active).contains(key);
    let deals = lk(&core.inner.deals);
    let v = rides
        .into_iter()
        .map(|r| {
            let step = deals.get(&r.deal_id).map(|d| d.step);
            (r.deal_id, step)
        })
        .collect();
    (v, active)
}

/// Сделка, которая держит монету и уже не на пути (окончания не будет): текст причины. Ошибок нет.
pub(super) fn stuck_of(list: &[(String, Option<Step>)]) -> Option<String> {
    list.iter()
        .find(|(_, s)| !matches!(s, Some(Step::Transfer | Step::Selling)))
        .map(|(id, s)| {
            let step = s.map_or("нет в учёте", super::deal_text::step_name);
            format!(
                "монету держит сделка {id} (шаг: {step}) — окончания не будет, сначала решите её"
            )
        })
}

/// Список «чьих окончаний ждём» для экрана. Ошибок нет.
pub(super) fn waits_for(list: &[(String, Option<Step>)], active: bool) -> Vec<String> {
    let mut v: Vec<String> = list.iter().map(|(id, _)| id.clone()).collect();
    if active {
        v.push("вход сделки или другое действие с монетой".into());
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
    if let Some(why) = stuck_of(&list) {
        return Err(format!("{old}: {why} (решение 405)"));
    }
    let what = cmd.action.title().to_string();
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
    };
    {
        let mut m = lk(&core.inner.manual_waits);
        if let Some(x) = m.get(key) {
            return Err(format!(
                "по {ticker} уже ждёт ручное действие «{}» (с {}) — отмените его («Отменить ожидание» на «Ручное \
                 управление») или дождитесь",
                x.what, x.since
            ));
        }
        m.insert(key.clone(), w.clone());
        core.inner.manual_waits_live.fetch_add(1, Ordering::AcqRel);
    }
    let text = format!(
        "«{what}»: ждёт окончания сделок по {ticker}: {} — новые сделки {ticker} на паузе; выполню после их окончания \
         (отменить — «Отменить ожидание» на «Ручное управление»)",
        w.waits_for.join(", ")
    );
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
            format!("«{}» по {}", w.what, w.coin)
        })
    };
    core.inner.coin_wake.notify_waiters();
    Some(match found {
        Some(t) => Ok(format!(
            "ожидание {t} отменено — действие не выполняется, пауза снимается"
        )),
        None => Err(format!("ожидания {id} нет (уже выполнено или отменено)")),
    })
}

/// Что делает: ожидания для экрана (GET /api/manual — waits). Ошибок нет.
pub fn list(core: &Core) -> Vec<ManualWait> {
    let mut v: Vec<ManualWait> = lk(&core.inner.manual_waits).values().cloned().collect();
    v.sort_by(|a, b| a.since.cmp(&b.since));
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
