//! Файл: core/candidates.rs
//! Что делает: отдельная очередь тяжёлого расчёта (ТЗ: «шаг 4 идёт отдельной очередью и только по
//! прошедшим фильтр»). Кандидат от signal.rs попадает в очередь один раз, пока не посчитан; расчёт —
//! profit.rs. Итог — на «Живой экран» (событие candidate); прошедший — в журнал, Telegram
//! (deal.candidate) и на исполнение (executor.rs); отказ — в журнал с причиной (повтор той же причины
//! по направлению не пишется, чтобы журнал не тонул).
//! Кто вызывает: core/tick.rs (enqueue), core/engine.rs (задача run).
//! От чего зависит: engine.rs, profit.rs, view.rs.
//! Настройки: через profit.rs.

use serde_json::json;
use tokio::sync::mpsc;

use crate::core::types::Direction;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
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
        // Расчёт кандидата — путь сделки: котировки площадок идут классом Trade (gateway/urgency.rs, решение 61).
        let verdict = urgency::scope(Urgency::Trade, profit::decide(&core, &d)).await;
        lk(&core.inner.queued).remove(&d);
        let row = json!({ "direction": d, "verdict": verdict, "at": chrono::Utc::now() });
        lk(&core.inner.candidates).insert(d.clone(), row.clone());
        core.inner.live.send(CoreEvent::Candidate(row));
        let deps = &core.inner.deps;
        let pair = format!("{} {} → {}", d.asset.ticker, d.buy_on.0, d.sell_on.0);
        match verdict {
            Verdict::Accept(calc) => {
                lk(&core.inner.last_reject).remove(&d);
                // Решение 66: почему этот объём — в тексте журнала и Telegram.
                let why = calc.size.as_ref().map(|s| s.why.as_str()).unwrap_or("");
                let text = format!(
                    "{pair}: объём {}, прибыль {} USDT ({}%); {why}",
                    calc.qty.round_dp(8),
                    calc.profit_usd.round_dp(4),
                    calc.profit_pct.round_dp(4)
                );
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
                super::executor::accepted(&core, calc).await;
            }
            Verdict::Reject { reason, best } => {
                let same = lk(&core.inner.last_reject).insert(d.clone(), reason.clone())
                    == Some(reason.clone());
                if !same {
                    deps.log(
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
