//! Файл: core/ledger_load.rs
//! Что делает: загрузка сделок при запуске (вынесено из ledger.rs): незакрытые — «прервана перезапуском», шаг Halted,
//! уведомление risk.stop и оценка позиции в итог дня (решения 53, 152); ждущие решения человека остаются ждать; у закрытой
//! сделки возврат USDT «идёт» — задача потеряна вместе с памятью: «не вернулись» со страховкой (решение 138).
//! Решение 236: у закрытой сделки затраты возврата USDT вносятся в итог (deal_return_cost::apply; старые записи — в
//! памяти, без записи в базу); отложенное deal.closed, если возврат больше не идёт, — отправляется.
//! Решение 394: закрытая сделка группы докупки, чья выручка ждала общего возврата группы (Deal.group.wait), — «не
//! вернулись» со страховкой (ожидание жило в памяти). Ревью 3, Н2: если её ID есть в GroupSum.deals сохранённого
//! носителя общего возврата (процесс упал после сохранения носителя, до сохранения остальных — coin_group_flush.rs),
//! её выручка — в общем возврате носителя: «общий возврат в сделке …», без «не вернулись» (носитель числит всю сумму).
//! Кто вызывает: engine.rs (через ledger::load).
//! От чего зависит: ledger.rs (save, KIND), persist.rs, daily.rs, daily_value.rs, usdt_return_state.rs, deal.rs,
//! deal_return_cost.rs, deal_closed.rs, group_notice.rs.
//! Настройки: нет.

use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};
use super::ledger::{KIND, recompute_exposure, save};

/// Начало строки хронологии «прервана перезапуском» — момент остановки сделки (решение 257: отчёт «Итоги сделок» берёт
/// по ней время конца сделки, report/rows_time.rs).
pub const INTERRUPTED: &str = "Сделка прервана перезапуском";

/// Загружает сделки при запуске. Незакрытые — «прервана перезапуском», шаг Halted, уведомление.
pub async fn load(core: &Core) {
    let deps = &core.inner.deps;
    let deals = match super::persist::load::<Deal>(&deps.db, KIND).await {
        Ok(d) => d,
        Err(e) => {
            deps.log(
                Level::Error,
                "core.ledger",
                None,
                format!("Сделки не загружены: {e}"),
                None,
            )
            .await;
            return;
        }
    };
    // Ревью 3, Н2: сделки, чья выручка вошла в общий возврат сохранённого носителя: ID → носитель.
    let shared: std::collections::HashMap<String, String> = deals
        .iter()
        .filter_map(|x| x.group.as_ref().map(|g| (x, g)))
        .filter(|(_, g)| g.wait.is_none())
        .filter_map(|(x, g)| g.sum.as_ref().map(|s| (x, s)))
        .flat_map(|(x, s)| s.deals.iter().map(move |id| (id.clone(), x.id.clone())))
        .collect();
    for mut d in deals {
        // Ждущие решения человека остаются ждать (алерты загружает manual.rs).
        if d.is_open() && d.step != Step::WaitingHuman {
            let text = format!(
                "{INTERRUPTED} (шаг: {}, {}) — проверьте вручную",
                super::deal_text::step_name(d.step),
                super::deal_text::coins_at(&d)
            );
            let tg = super::deal_text::interrupted(&d);
            d.step = Step::Halted;
            d.note(text.clone());
            // Решения 53, 152: остановленная сделка — оценка позиции в итог дня (цены ещё нет — худший случай).
            // При запуске рыночных данных ещё нет (стаканы и пулы не загружены), а котировка DEX пошла бы на узел и
            // задержала запуск — оценка сразу «цены нет».
            let est =
                super::daily_value::estimate(&d, Err("перезапуск: рыночных данных ещё нет".into()));
            let trip = super::daily::record(core, &mut d, est).await;
            super::daily_commit::announce(core, &trip).await;
            deps.log(Level::Warn, "core.ledger", Some(&d.at.0), &text, None)
                .await;
            deps.notify("risk.stop", Some(&d.at.0), &tg).await;
            save(core, &d).await;
        } else if super::usdt_return_state::on_restart(core, &mut d).await {
            // Решение 138: возврат USDT закрытой сделки прерван перезапуском — «не вернулись», страховка.
            super::deal_return_cost::apply(&mut d);
            let notice = super::deal_closed::take_ready(&mut d);
            // Решение 394 (ревью 3, Н4): «Итог группы» носителя, уже известившего о закрытии.
            let total = super::group_notice::take(&mut d);
            save(core, &d).await;
            if let Some(why) = notice {
                super::deal_closed::send(core, &d, &why).await;
            }
            if let Some(t) = total {
                super::group_notice::send(core, &d, &t).await;
            }
        } else if d.group.as_ref().is_some_and(|g| g.wait.is_some())
            && let Some(bearer) = shared.get(&d.id)
        {
            // Ревью 3, Н2: выручка уже в общем возврате носителя (он сохранён и числит всю сумму).
            if let Some(g) = d.group.as_mut() {
                g.wait = None;
                g.shared_in = Some(bearer.clone());
            }
            d.note(format!(
                "Возврат USDT: 0 — общий возврат группы в сделке {bearer} (восстановлено при запуске, решение 394)"
            ));
            save(core, &d).await;
        } else if let Some((from, amount)) = d.group.as_ref().and_then(|g| g.wait.clone()) {
            // Решение 394: выручка сделки ждала общего возврата группы докупки (в памяти) — перезапуск его потерял:
            // «не вернулись» со страховкой, решает человек.
            if let Some(g) = d.group.as_mut() {
                g.wait = None;
            }
            let why = "перезапуск: общий возврат USDT группы докупки не состоялся (решение 394)";
            super::usdt_return_deal::not_returned(core, &mut d, amount, &from, why).await;
            super::deal_return_cost::apply(&mut d);
            let notice = super::deal_closed::take_ready(&mut d);
            save(core, &d).await;
            if let Some(why) = notice {
                super::deal_closed::send(core, &d, &why).await;
            }
        } else {
            // Решение 236: затраты возврата USDT — в итоге сделки; у записи до решения итог исправляется в памяти
            // (запись в базе — как была, до следующего сохранения сделки).
            super::deal_return_cost::apply(&mut d);
            lk(&core.inner.deals).insert(d.id.clone(), d);
        }
    }
    recompute_exposure(core);
}
