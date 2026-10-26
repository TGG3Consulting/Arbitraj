//! Файл: core/candidates.rs
//! Что делает: отдельная очередь тяжёлого расчёта (ТЗ: «шаг 4 идёт отдельной очередью и только по
//! прошедшим фильтр»). Кандидат от signal.rs попадает в очередь один раз, пока не посчитан; расчёт —
//! profit.rs. Итог — на «Живой экран» (событие candidate, у прошедшего — карточка сигнала card.rs и метка «сухой»);
//! прошедший — на исполнение (executor.rs; в сухом режиме — нет, решение 74), в эпизод (episode_run.rs), в журнал и
//! Telegram (deal.candidate) — при открытии эпизода; отказ — в журнал с причиной (повтор той же причины по направлению
//! не пишется, чтобы журнал не тонул) и закрывает эпизод направления.
//! Пересчёт открытого эпизода — не чаще episode.min_interval_ms (enqueue).
//! Кто вызывает: core/tick.rs (enqueue), core/engine.rs (задача run).
//! От чего зависит: engine.rs, profit.rs, dry.rs, executor.rs, card.rs, card_text.rs, episode.rs, episode_run.rs, view.rs.
//! Настройки: через profit.rs, dry.rs, card.rs, episode_cfg.rs.

use serde_json::json;
use tokio::sync::mpsc;

use crate::core::types::Direction;
use crate::storage::journal::Level;

use super::costs::Calc;
use super::engine::{Core, lk};
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
    // Открытый эпизод (решение 74): пересчёт не чаще episode.min_interval_ms, чтобы не утонуть.
    let min_ms = core
        .inner
        .deps
        .settings
        .integer("episode.min_interval_ms", Default::default())
        .unwrap_or_default();
    if !lk(&core.inner.episodes).due(d, chrono::Utc::now(), min_ms) {
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
        // Расчёт кандидата — путь сделки: котировки площадок идут классом Trade (gateway/urgency.rs, решение 61);
        // пересчёт открытого эпизода (решение 74) — классом Quote: это наблюдение, не сделка.
        let class = if super::episode_run::is_open(&core, &d) {
            Urgency::Quote
        } else {
            Urgency::Trade
        };
        let verdict = urgency::scope(class, profit::decide(&core, &d)).await;
        lk(&core.inner.queued).remove(&d);
        match verdict {
            Verdict::Accept(calc) => accepted(&core, &d, calc).await,
            Verdict::Reject { reason, best } => {
                let row = json!({ "direction": d, "verdict": Verdict::Reject { reason: reason.clone(), best: best.clone() }, "at": chrono::Utc::now() });
                publish(&core, &d, row);
                // Направление перестало проходить — эпизод закрывается с этой причиной (решение 74).
                super::episode_run::failed(&core, &d, &reason).await;
                lk(&core.inner.dry_told).remove(&d);
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

/// Прошедший кандидат: обычный режим — сразу исполнитель (до карточки: сделка не ждёт тяжёлого расчёта);
/// сухой (решение 74, dry.rs) — исполнитель не вызывается. Затем карточка сигнала (card.rs), эпизод (episode_run.rs),
/// «Живой экран». Журнал и Telegram (deal.candidate, текст карточки) — при открытии эпизода; эпизоды не ведутся (не
/// заданы episode.*) — на каждый прошедший, как до решения 74 (в сухом режиме — один раз, пока направление проходит:
/// inner.dry_told). Ошибок нет.
async fn accepted(core: &Core, d: &Direction, calc: Calc) {
    lk(&core.inner.last_reject).remove(d);
    let start = match super::dry::blocks(&core.inner.deps.settings) {
        Some(why) => Start::Dry(why),
        None => super::executor::accepted(core, calc.clone()).await,
    };
    let dry = matches!(start, Start::Dry(_));
    let card = urgency::scope(Urgency::Quote, super::card::build(core, &calc, dry)).await;
    let row = json!({
        "direction": d,
        "verdict": Verdict::Accept(calc.clone()),
        "at": chrono::Utc::now(),
        "dry": dry,
        "card": card,
    });
    publish(core, d, row);
    let opened = super::episode_run::accepted(core, &calc, &card, dry).await;
    if opened == Some(false) {
        return;
    }
    // Сухой режим без эпизодов: пересчёт идёт на каждом тике — сообщаем один раз, пока направление проходит.
    if opened.is_none() && dry && !lk(&core.inner.dry_told).insert(d.clone()) {
        return;
    }
    let mut text = super::card_text::text(&card);
    if let Some(s) = calc.size.as_ref() {
        // Решение 66: почему этот объём.
        text.push_str(&format!("\nОбъём: {}", s.why));
    }
    if let Start::Dry(why) = &start {
        text.push_str(&format!("\n{why}"));
    }
    let deps = &core.inner.deps;
    deps.log(
        Level::Info,
        "core.candidate",
        Some(&d.buy_on.0),
        &text,
        serde_json::to_value(&calc).ok(),
    )
    .await;
    deps.notify("deal.candidate", Some(&d.buy_on.0), &text)
        .await;
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
