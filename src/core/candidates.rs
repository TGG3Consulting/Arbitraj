//! Файл: core/candidates.rs
//! Что делает: отдельная очередь тяжёлого расчёта (ТЗ: «шаг 4 идёт отдельной очередью и только по
//! прошедшим фильтр»). Расчёты идут ПАРАЛЛЕЛЬНО — на каждый кандидат свой (решение 88): тяжёлый счёт в памяти не
//! нагружает узел (цена газа — общий кэш на блок gas.rs, пулы из стрима), поэтому ни один расчёт не ждёт другого;
//! предел одновременных — profit.calc_concurrency (0 / не задана — без предела, достигнут — направление снимается из
//! очереди и встанет снова на следующем тике), общий срок одного расчёта — profit.calc_timeout_ms (не даёт зависнуть на
//! медленном узле, отмена decide безопасна — блокировки через await не держатся; не задан — без срока).
//! Кандидат от signal.rs попадает в очередь один раз, пока не посчитан; расчёт —
//! profit.rs, класс Trade, как всегда (эпизоды на торговый путь не влияют, решение 74). Режим (сухой / обычный)
//! фиксируется один раз на расчёт и передаётся явно (dry.rs). Итог — на «Живой экран» (событие candidate, у прошедшего —
//! метка «сухой»); прошедший — на исполнение (executor.rs; в сухом режиме — нет), в эпизод (episode_run.rs — только
//! память и сообщение), карточка сигнала — отдельной задачей (card_job.rs), очередь её не ждёт; отказ — в журнал с
//! причиной (повтор того же вида причины — счётчиком в той же строке, решение 155) и закрывает эпизод направления. Окно монеты (решение 125,
//! window_run.rs): прошедший открывает его или входит в него ДО исполнителя; отказ — «направление не проходит».
//! Журнал и Telegram deal.candidate: эпизоды ведутся — при открытии эпизода (episode_notify.rs); не ведутся — на каждый
//! прошедший, в сухом режиме — один раз, пока направление проходит (inner.dry_told).
//! Решение 129: каждый расчёт замеряется по этапам (calc_timing.rs: очередь до старта, этапы и части, запросы к узлу и
//! агрегатору) — поле `timing` строки «Кандидатов»; отказ по сроку называет этап, на котором расчёт стоит (calc_report.rs),
//! повтор той же причины в журнал не пишется по ключу без чисел.
//! Решение 155: core.candidate_rejected — одна строка на смену вида причины по направлению (числа в сравнение не входят),
//! повторы — счётчиком и временем последнего в той же строке (reject_log.rs, частота обновления journal.repeat_flush_ms;
//! не задана — прежний режим, строка на каждый отказ с другим текстом); прошёл — строка закрывается итоговым счётчиком.
//! Решение 147: исполнитель отказал на входе (направление не торгуемо, по монете занято) — точная причина в поле
//! `start` строки «Кандидатов», в тексте карточки (журнал/Telegram deal.candidate) и в журнале core.start_refused
//! (Warn, одна запись на смену причины по направлению; сделка запущена — память причины стирается).
//! Решение 267: монета занята сделкой — при window.busy_calc = да вместо молчаливого выхода enqueue ставит теневой
//! расчёт (busy_calc.rs: только наблюдение — ни исполнителя, ни окна, ни эпизода, ни строки «Кандидатов», ни журнала
//! отказов); его очередь разбирает run после очереди кандидатов. Выключено / не задано — как раньше.
//! Решение 141: прошедший без запущенной сделки и отклонённый с расчётом — в фоновый расчёт «без потолка»
//! (uncapped_entry.rs, очередь не ждёт); прежний итог «без потолка» в строке остаётся до нового.
//! Кто вызывает: core/tick.rs (enqueue), core/engine.rs (задача run), тесты (accepted).
//! От чего зависит: engine.rs, profit.rs, dry.rs, executor.rs, card_job.rs, episode_run.rs, view.rs, calc_timing.rs,
//! calc_report.rs, reject_log.rs, reject_shape.rs, gateway/calc_meter.rs, busy_calc.rs (решение 267).
//! Настройки: через profit.rs, dry.rs, card.rs, episode_cfg.rs; journal.repeat_flush_ms (решение 155).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::core::types::Direction;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::calc_report;
use super::calc_timing::{self, Timing};
use super::card_job::{Job, submit};
use super::costs::Calc;
use super::dry::Mode;
use super::engine::{Core, lk};
use super::episode_run::Accepted;
use super::executor::Start;
use super::profit::{self, Verdict};
use super::reject_log::Note;
use super::reject_shape;
use super::view::CoreEvent;
use crate::gateway::calc_meter;
use crate::gateway::urgency::{self, Urgency};

/// Ставит направление в очередь расчёта (если его там ещё нет). Очередь полна — пропуск
/// (следующий тик поставит снова). Ошибок нет.
pub fn enqueue(core: &Core, d: &Direction) {
    // По монете уже идёт сделка — новый расчёт не нужен (решение 118, Я-2: одна сделка на монету).
    if lk(&core.inner.active).contains(&super::coin_key::CoinKey::of(&d.asset)) {
        // Решение 267: включён замер window.busy_calc — теневой расчёт (только наблюдение, своя очередь); выключен
        // или не задан — одно чтение настройки и тот же молчаливый выход.
        super::busy_calc::enqueue(core, d);
        return;
    }
    if !lk(&core.inner.queued).insert(d.clone()) {
        return;
    }
    if core
        .inner
        .cand_tx
        .try_send((d.clone(), Instant::now()))
        .is_err()
    {
        lk(&core.inner.queued).remove(d);
    }
}

/// Задача очереди: раздаёт расчёты параллельно — на каждый кандидат свой расчёт (решение 88), ни один не ждёт другого.
/// Предел одновременных расчётов — profit.calc_concurrency (0 или не задана — без предела); достигнут — направление
/// снимается из очереди, следующий тик поставит снова (как при полном канале). Работает, пока жив канал.
/// Решение 267: та же задача разбирает очередь теневых расчётов занятых монет (busy_calc.rs Shadows) — под тем же
/// пределом и только когда очередь кандидатов пуста; условие допуска кандидата прежнее (теневые в inflight не входят).
pub async fn run(core: Core, mut rx: mpsc::Receiver<(Direction, Instant)>) {
    let inflight = Arc::new(AtomicUsize::new(0));
    // Решение 267: теневые расчёты занятых монет — своя очередь; берётся, только когда очередь кандидатов пуста
    // (biased: кандидат всегда первый). Замер выключен — очередь теневых пуста всегда.
    let mut shadows = super::busy_calc::Shadows::take(&core);
    loop {
        let (d, at) = tokio::select! {
            biased;
            m = rx.recv() => match m {
                Some(m) => m,
                None => return,
            },
            s = shadows.recv() => {
                shadows.start(&core, s, inflight.load(Ordering::Acquire));
                continue;
            }
        };
        let cap = core
            .inner
            .deps
            .settings
            .integer("profit.calc_concurrency", At::default())
            .ok()
            .filter(|n| *n > 0)
            .map(|n| n as usize);
        if cap.is_some_and(|c| inflight.load(Ordering::Acquire) >= c) {
            // Предел одновременных расчётов достигнут — не ждём: снять из очереди, следующий тик поставит снова.
            lk(&core.inner.queued).remove(&d);
            continue;
        }
        inflight.fetch_add(1, Ordering::AcqRel);
        // Решение 267: кандидат занял место — лишний теневой расчёт снимается (теневых нет — ничего).
        shadows.give_way(cap, inflight.load(Ordering::Acquire));
        let core = core.clone();
        let inflight = inflight.clone();
        // Свой расчёт на каждый кандидат: тяжёлый счёт (в памяти) идёт параллельно, узел не нагружается
        // (цена газа — общий кэш на блок, пулы из стрима). Класс Trade ставится внутри задачи (task_local не наследуется).
        tokio::spawn(async move {
            calc_one(&core, &d, at).await;
            lk(&core.inner.queued).remove(&d);
            inflight.fetch_sub(1, Ordering::AcqRel);
        });
    }
}

/// Один расчёт кандидата. Режим и общий срок фиксируются на расчёт. Срок profit.calc_timeout_ms не даёт расчёту
/// зависнуть (отмена decide безопасна — блокировки через await не держатся); не задан — без срока. Замер этапов —
/// решение 129. Принимает: ядро, направление, момент постановки в очередь. Ошибок нет — итог в «Кандидаты» и журнал.
async fn calc_one(core: &Core, d: &Direction, queued_at: Instant) {
    // Режим — один раз на расчёт: бюджет считается в нём же, решение по исполнителю — тоже (решение 74).
    let mode = super::dry::mode(&core.inner.deps.settings);
    let t = Timing::new(queued_at);
    // Расчёт кандидата — путь сделки: котировки площадок идут классом Trade (gateway/urgency.rs, решение 61).
    let work = urgency::scope(
        Urgency::Trade,
        calc_meter::scope(
            t.meter.clone(),
            calc_timing::scope(t.clone(), profit::decide(core, d, &mode)),
        ),
    );
    // Ключ повтора причины в журнале: у отказа по сроку — без чисел замера (иначе каждый повтор — новая строка).
    let mut key = None;
    let verdict = match core
        .inner
        .deps
        .settings
        .integer("profit.calc_timeout_ms", At::default())
        .ok()
        .filter(|ms| *ms > 0)
    {
        Some(ms) => match timeout(Duration::from_millis(ms), work).await {
            Ok(v) => v,
            Err(_) => {
                // Решение 129: причина называет этап, на котором расчёт стоит, и сколько длились прошедшие.
                let (reason, k) = calc_report::timeout_reason(ms, &calc_report::report(&t));
                key = Some(k);
                Verdict::Reject { reason, best: None }
            }
        },
        None => work.await,
    };
    let timing = serde_json::to_value(calc_report::report(&t)).unwrap_or_default();
    match verdict {
        Verdict::Accept(calc) => accepted_timed(core, d, calc, &mode, timing).await,
        Verdict::Reject { reason, best } => {
            let row = json!({ "direction": d, "verdict": Verdict::Reject { reason: reason.clone(), best: best.clone() }, "at": chrono::Utc::now(), "timing": timing });
            publish(core, d, row);
            // Решение 141: отказ с расчётом — фоновый расчёт «без потолка» (путь не ждёт).
            if let Some(b) = &best {
                super::uncapped_entry::cand(core, d, b, Some(&reason), &mode);
            }
            // Направление перестало проходить — эпизод закрывается с этой причиной (решение 74).
            super::episode_run::failed(core, d, &reason);
            // Решение 155: одна строка журнала на смену вида причины (без чисел), повторы — счётчиком в ней же.
            let exact = key.unwrap_or_else(|| reason.clone());
            let pair = format!("{} {} → {}", d.asset.ticker, d.buy_on.0, d.sell_on.0);
            let note = Note {
                level: Level::Info,
                kind: "core.candidate_rejected",
                venue: Some(&d.buy_on.0),
                shape: reject_shape::shape(&exact),
                exact,
                message: format!("{pair}: отказ — {reason}"),
                data: best.and_then(|b| serde_json::to_value(b).ok()),
            };
            let deps = &core.inner.deps;
            core.inner
                .last_reject
                .note(&deps.journal, &reject_key(d), note, flush_every(core))
                .await;
        }
    }
}

/// Что делает: прошедший кандидат (см. шапку). Обычный режим — сразу исполнитель; сухой (режим расчёта) — исполнитель
/// не вызывается. Затем строка «Кандидатов», эпизод (память и сообщение), карточка — отдельной задачей.
/// Принимает: ядро, направление, расчёт, режим, в котором он посчитан. Ошибок нет. Без замера этапов (тесты).
pub async fn accepted(core: &Core, d: &Direction, calc: Calc, mode: &Mode) {
    accepted_timed(core, d, calc, mode, serde_json::Value::Null).await
}

/// То же с замером этапов расчёта (решение 129) — поле `timing` строки. Ошибок нет.
async fn accepted_timed(
    core: &Core,
    d: &Direction,
    calc: Calc,
    mode: &Mode,
    timing: serde_json::Value,
) {
    // Решение 155: кандидат прошёл — строка отказа закрывается итоговым счётчиком.
    core.inner
        .last_reject
        .close(&core.inner.deps.journal, &reject_key(d))
        .await;
    // Решение 125: окно монеты — до исполнителя, чтобы запись на биржах началась раньше сделки (только память).
    super::window_run::pass(core, &calc, matches!(mode, Mode::Dry(_)));
    let start = match mode {
        Mode::Dry(_) => {
            Start::Dry("сухой режим (dry.enabled): ордера и переводы не выполняются".into())
        }
        Mode::Live => super::executor::accepted(core, calc.clone()).await,
    };
    let dry = matches!(start, Start::Dry(_));
    // Решение 147: отказ исполнителя на входе — причина в строку, журнал (на смену причины) и текст карточки.
    let refused = match &start {
        Start::NotTradable(why) => Some(format!("сделки нет — направление не торгуемо: {why}")),
        Start::Busy(why) => Some(format!("сделки нет — по монете занято: {why}")),
        _ => None,
    };
    refused_log(core, d, &start, refused.as_deref()).await;
    // Решение 141: сделка не запущена — расчёт «без потолка» кандидата; запущена — его ставит исполнитель после покупки.
    if start != Start::Started {
        super::uncapped_entry::cand(core, d, &calc, None, mode);
    }
    // Прежняя карточка остаётся видна, пока card_job.rs не построит новую.
    let prev = lk(&core.inner.candidates)
        .get(d)
        .map(|r| r["card"].clone())
        .unwrap_or_default();
    let row = json!({
        "direction": d,
        "verdict": Verdict::Accept(calc.clone()),
        "at": chrono::Utc::now(),
        "dry": dry,
        "card": prev,
        "timing": timing,
        "start": refused.clone(),
    });
    publish(core, d, row);
    let ep = super::episode_run::on_accept(core, &calc, dry);
    let (told, wait) = match &ep {
        Accepted::Off => (!dry || lk(&core.inner.dry_told).insert(d.clone()), true),
        Accepted::Skip => (false, false),
        Accepted::Ep { .. } => (false, true),
    };
    let note = match start {
        Start::Dry(why) => Some(why),
        _ => refused,
    };
    submit(
        core,
        Job {
            calc,
            dry,
            ep,
            told,
            note,
        },
        wait,
    );
}

/// Решение 147: журнал core.start_refused (Warn) — одна запись на смену причины по направлению; сделка запущена —
/// память причины стирается (следующий отказ снова пишется). Принимает: ядро, направление, итог входа, текст отказа
/// (None — не отказ). Ошибок нет.
async fn refused_log(core: &Core, d: &Direction, start: &Start, refused: Option<&str>) {
    let Some(why) = refused else {
        if *start == Start::Started {
            lk(&core.inner.start_refused).remove(d);
        }
        return;
    };
    let same = lk(&core.inner.start_refused)
        .insert(d.clone(), why.to_string())
        .as_deref()
        == Some(why);
    if same {
        return;
    }
    let text = format!(
        "{} {} → {}: расчёт прошёл, {why}",
        d.asset.ticker, d.buy_on.0, d.sell_on.0
    );
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.start_refused",
            Some(&d.buy_on.0),
            text,
            None,
        )
        .await;
}

/// Решение 155: ключ строки отказа в журнале — направление целиком. Ошибок нет.
pub(super) fn reject_key(d: &Direction) -> String {
    serde_json::to_string(d).unwrap_or_else(|_| format!("{d:?}"))
}

/// Решение 155: journal.repeat_flush_ms — как часто обновлять строку повторов (0 — каждый повтор); не задана — None
/// (прежний режим: новая строка на каждый отказ с другим текстом). Ошибок нет.
pub(super) fn flush_every(core: &Core) -> Option<Duration> {
    let s = &core.inner.deps.settings;
    s.integer("journal.repeat_flush_ms", At::default())
        .ok()
        .map(Duration::from_millis)
}

/// Строка «Кандидатов» — в память и живой поток. Прежний итог «без потолка» (решение 141, поле uncapped) остаётся
/// виден, пока фоновый расчёт не даст новый. Ошибок нет.
fn publish(core: &Core, d: &Direction, mut row: serde_json::Value) {
    {
        let mut rows = lk(&core.inner.candidates);
        if let Some(u) = rows.get(d).and_then(|r| r.get("uncapped")).cloned() {
            row["uncapped"] = u;
        }
        rows.insert(d.clone(), row.clone());
    }
    core.inner.live.send(CoreEvent::Candidate(row));
}
