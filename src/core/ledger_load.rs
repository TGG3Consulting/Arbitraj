//! Файл: core/ledger_load.rs
//! Что делает: загрузка сделок при запуске (вынесено из ledger.rs): незакрытые — «прервана перезапуском», шаг Halted,
//! уведомление risk.stop и предварительный убыток (решение 53); ждущие решения человека остаются ждать; у закрытой
//! сделки возврат USDT «идёт» — задача потеряна вместе с памятью: «не вернулись» со страховкой (решение 138).
//! Кто вызывает: engine.rs (через ledger::load).
//! От чего зависит: ledger.rs (save, KIND), persist.rs, daily.rs, usdt_return_state.rs, deal.rs.
//! Настройки: нет.

use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};
use super::ledger::{KIND, recompute_exposure, save};

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
    for mut d in deals {
        // Ждущие решения человека остаются ждать (алерты загружает manual.rs).
        if d.is_open() && d.step != Step::WaitingHuman {
            let text = format!(
                "Сделка {} ({}, шаг {:?}, {} монет на {}) прервана перезапуском — проверьте вручную",
                d.id, d.direction.asset.ticker, d.step, d.qty, d.at.0
            );
            d.step = Step::Halted;
            d.note(text.clone());
            // Решение 53: остановленная сделка — предварительный убыток в итог дня.
            if let Some(stop) = super::daily::provisional(core, &mut d).await {
                deps.notify("risk.daily_loss", None, &stop).await;
            }
            deps.log(Level::Warn, "core.ledger", Some(&d.at.0), &text, None)
                .await;
            deps.notify("risk.stop", Some(&d.at.0), &text).await;
            save(core, &d).await;
        } else if super::usdt_return_state::on_restart(core, &mut d).await {
            // Решение 138: возврат USDT закрытой сделки прерван перезапуском — «не вернулись», страховка.
            save(core, &d).await;
        } else {
            lk(&core.inner.deals).insert(d.id.clone(), d);
        }
    }
    recompute_exposure(core);
}
