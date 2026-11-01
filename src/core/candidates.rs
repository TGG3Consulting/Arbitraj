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
//! причиной (повтор той же причины по направлению не пишется) и закрывает эпизод направления.
//! Журнал и Telegram deal.candidate: эпизоды ведутся — при открытии эпизода (episode_notify.rs); не ведутся — на каждый
//! прошедший, в сухом режиме — один раз, пока направление проходит (inner.dry_told).
//! Кто вызывает: core/tick.rs (enqueue), core/engine.rs (задача run), тесты (accepted).
//! От чего зависит: engine.rs, profit.rs, dry.rs, executor.rs, card_job.rs, episode_run.rs, view.rs.
//! Настройки: через profit.rs, dry.rs, card.rs, episode_cfg.rs.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::core::types::Direction;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::card_job::{Job, submit};
use super::costs::Calc;
use super::dry::Mode;
use super::engine::{Core, lk};
use super::episode_run::Accepted;
use super::executor::Start;
use super::profit::{self, Verdict};
use super::view::CoreEvent;
use crate::gateway::urgency::{self, Urgency};

/// Ставит направление в очередь расчёта (если его там ещё нет). Очередь полна — пропуск
/// (следующий тик поставит снова). Ошибок нет.
pub fn enqueue(core: &Core, d: &Direction) {
    // По направлению уже идёт сделка — новый расчёт не нужен (одна сделка на направление).
    if lk(&core.inner.active).contains(d) {
        return;
    }
    if !lk(&core.inner.queued).insert(d.clone()) {
        return;
    }
    if core.inner.cand_tx.try_send(d.clone()).is_err() {
        lk(&core.inner.queued).remove(d);
    }
}

/// Задача очереди: раздаёт расчёты параллельно — на каждый кандидат свой расчёт (решение 88), ни один не ждёт другого.
/// Предел одновременных расчётов — profit.calc_concurrency (0 или не задана — без предела); достигнут — направление
/// снимается из очереди, следующий тик поставит снова (как при полном канале). Работает, пока жив канал.
pub async fn run(core: Core, mut rx: mpsc::Receiver<Direction>) {
    let inflight = Arc::new(AtomicUsize::new(0));
    while let Some(d) = rx.recv().await {
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
        let core = core.clone();
        let inflight = inflight.clone();
        // Свой расчёт на каждый кандидат: тяжёлый счёт (в памяти) идёт параллельно, узел не нагружается
        // (цена газа — общий кэш на блок, пулы из стрима). Класс Trade ставится внутри задачи (task_local не наследуется).
        tokio::spawn(async move {
            calc_one(&core, &d).await;
            lk(&core.inner.queued).remove(&d);
            inflight.fetch_sub(1, Ordering::AcqRel);
        });
    }
}

/// Один расчёт кандидата. Режим и общий срок фиксируются на расчёт. Срок profit.calc_timeout_ms не даёт расчёту
/// зависнуть, когда узел медлит (отмена decide безопасна — блокировки через await не держатся); не задан — без срока.
/// Принимает: ядро, направление. Ошибок нет — итог в строку «Кандидатов» и журнал.
async fn calc_one(core: &Core, d: &Direction) {
    // Режим — один раз на расчёт: бюджет считается в нём же, решение по исполнителю — тоже (решение 74).
    let mode = super::dry::mode(&core.inner.deps.settings);
    // Расчёт кандидата — путь сделки: котировки площадок идут классом Trade (gateway/urgency.rs, решение 61).
    let work = urgency::scope(Urgency::Trade, profit::decide(core, d, &mode));
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
            Err(_) => Verdict::Reject {
                reason: format!("расчёт не уложился в срок {ms} мс — узел медлит или перегружен"),
                best: None,
            },
        },
        None => work.await,
    };
    match verdict {
        Verdict::Accept(calc) => accepted(core, d, calc, &mode).await,
        Verdict::Reject { reason, best } => {
            let row = json!({ "direction": d, "verdict": Verdict::Reject { reason: reason.clone(), best: best.clone() }, "at": chrono::Utc::now() });
            publish(core, d, row);
            // Направление перестало проходить — эпизод закрывается с этой причиной (решение 74).
            super::episode_run::failed(core, d, &reason);
            let same = lk(&core.inner.last_reject).insert(d.clone(), reason.clone())
                == Some(reason.clone());
            if !same {
                let pair = format!("{} {} → {}", d.asset.ticker, d.buy_on.0, d.sell_on.0);
                core.inner
                    .deps
                    .log(
                        Level::Info,
                        "core.candidate_rejected",
                        Some(&d.buy_on.0),
                        format!("{pair}: отказ — {reason}"),
                        best.and_then(|b| serde_json::to_value(b).ok()),
                    )
                    .await;
            }
        }
    }
}

/// Что делает: прошедший кандидат (см. шапку). Обычный режим — сразу исполнитель; сухой (режим расчёта) — исполнитель
/// не вызывается. Затем строка «Кандидатов», эпизод (память и сообщение), карточка — отдельной задачей.
/// Принимает: ядро, направление, расчёт, режим, в котором он посчитан. Ошибок нет.
pub async fn accepted(core: &Core, d: &Direction, calc: Calc, mode: &Mode) {
    lk(&core.inner.last_reject).remove(d);
    let start = match mode {
        Mode::Dry(_) => {
            Start::Dry("сухой режим (dry.enabled): ордера и переводы не выполняются".into())
        }
        Mode::Live => super::executor::accepted(core, calc.clone()).await,
    };
    let dry = matches!(start, Start::Dry(_));
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
        _ => None,
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

/// Строка «Кандидатов» — в память и живой поток. Ошибок нет.
fn publish(core: &Core, d: &Direction, row: serde_json::Value) {
    lk(&core.inner.candidates).insert(d.clone(), row.clone());
    core.inner.live.send(CoreEvent::Candidate(row));
}

impl Core {
    /// Последние расчёты кандидатов по направлениям (новые сверху) — «Живой экран».
    pub fn candidates(&self) -> serde_json::Value {
        let mut v: Vec<serde_json::Value> = lk(&self.inner.candidates).values().cloned().collect();
        v.sort_by(|a, b| b["at"].as_str().cmp(&a["at"].as_str()));
        serde_json::Value::Array(v)
    }

    /// Последняя статистика каналов отправки (события адаптеров DEX) — экран «Каналы отправки».
    pub fn channel_stats(&self) -> Vec<crate::core::types::SendChannelStats> {
        super::engine::rd(&self.inner.market).channels()
    }
}
