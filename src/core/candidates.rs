//! Файл: core/candidates.rs
//! Что делает: отдельная очередь тяжёлого расчёта (ТЗ: «шаг 4 идёт отдельной очередью и только по
//! прошедшим фильтр»). Кандидат от signal.rs попадает в очередь один раз, пока не посчитан; расчёт —
//! profit.rs. Итог — на «Живой экран» (событие candidate); прошедший — в журнал, Telegram
//! (deal.candidate); отказ — в журнал с причиной (повтор той же причины
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

/// Ставит направление в очередь расчёта (если его там ещё нет). Очередь полна — пропуск
/// (следующий тик поставит снова). Ошибок нет.
pub fn enqueue(core: &Core, d: &Direction) {
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
        let verdict = profit::decide(&core, &d).await;
        lk(&core.inner.queued).remove(&d);
        core.inner.live.send(CoreEvent::Candidate(
            json!({ "direction": d, "verdict": verdict }),
        ));
        let deps = &core.inner.deps;
        let pair = format!("{} {} → {}", d.asset.ticker, d.buy_on.0, d.sell_on.0);
        match verdict {
            Verdict::Accept(calc) => {
                lk(&core.inner.last_reject).remove(&d);
                let text = format!(
                    "{pair}: объём {}, прибыль {} USDT ({}%)",
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
