//! Файл: core/candidates.rs
//! Что делает: отдельная очередь тяжёлого расчёта (ТЗ: «шаг 4 идёт отдельной очередью и только по
//! прошедшим фильтр»). Кандидат от signal.rs попадает в очередь один раз, пока не посчитан; расчёт —
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

use serde_json::json;
use tokio::sync::mpsc;

use crate::core::types::Direction;
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

/// Задача очереди: считает кандидатов по одному. Работает, пока жив канал.
pub async fn run(core: Core, mut rx: mpsc::Receiver<Direction>) {
    while let Some(d) = rx.recv().await {
        // Режим — один раз на расчёт: бюджет считается в нём же, решение по исполнителю — тоже (решение 74).
        let mode = super::dry::mode(&core.inner.deps.settings);
        // Расчёт кандидата — путь сделки: котировки площадок идут классом Trade (gateway/urgency.rs, решение 61).
        let verdict = urgency::scope(Urgency::Trade, profit::decide(&core, &d, &mode)).await;
        lk(&core.inner.queued).remove(&d);
        match verdict {
            Verdict::Accept(calc) => accepted(&core, &d, calc, &mode).await,
            Verdict::Reject { reason, best } => {
                let row = json!({ "direction": d, "verdict": Verdict::Reject { reason: reason.clone(), best: best.clone() }, "at": chrono::Utc::now() });
                publish(&core, &d, row);
                // Направление перестало проходить — эпизод закрывается с этой причиной (решение 74).
                super::episode_run::failed(&core, &d, &reason);
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
