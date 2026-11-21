//! Файл: core/tick_slow.rs
//! Что делает: медленные события тика (решение 131) — очередь кошелька, конец прохода проверок, баланс, уведомление
//! площадки, замер выхода продажи и свежие налоги монеты (решение 248): они ждут журнал, Telegram или учёт, поэтому идут своей задачей по порядку (tick.rs), а цикл тика их не ждёт.
//! Кто вызывает: core/tick.rs.
//! От чего зависит: engine.rs, market_state.rs (Touched), queue_notify.rs, first_pass.rs, credit_event.rs, exit_gap.rs, deps.rs.
//! Настройки: нет своих.

use super::engine::Core;
use super::market_state::Touched;

/// Медленные события тика (журнал, Telegram, учёт) — по порядку, своей задачей (решение 129). Ошибок нет.
pub async fn slow(core: &Core, t: Touched) {
    match t {
        Touched::WalletQueue { queue, prev } => {
            super::queue_notify::wallet_queue(core, queue, prev).await
        }
        Touched::CheckPass {
            venue,
            counts: [ok, failed, waiting, config, loading],
        } => {
            let n = super::first_pass::Counts {
                ok,
                failed,
                waiting,
                config,
                loading,
            };
            super::first_pass::done(core, &venue.0, n).await
        }
        Touched::Balance(b) => super::credit_event::on_change(core, b).await,
        Touched::Notice { venue, code, text } => {
            // Решение 408: wallet.executor_v1 — «Развернуть исполнитель» у кошелька ЭТОЙ тревоги (ревью t193 М2: адрес его
            // исполнителя — в тексте тревоги, v1_notice.rs) с исполнителем версии v1 (версия
            // по сверке кода, settings/accounts executor_version; сначала оценка газа, потом «Да/Нет» — нажимает
            // только человек).
            let acts: Vec<crate::notify::acts::Act> = if code == "wallet.executor_v1" {
                core.inner
                    .deps
                    .settings
                    .accounts()
                    // Решение 432: исполнители площадки DEX этой тревоги (её сети).
                    .wallets_at(&venue.0)
                    .into_iter()
                    .filter(|w| {
                        w.executor_version.as_deref() == Some("v1")
                            && w.executor.as_deref().is_some_and(|x| text.contains(x))
                    })
                    .map(|w| crate::notify::acts::e::deploy_executor(&venue.0, &w.id, &w.name))
                    .collect()
            } else {
                Vec::new()
            };
            core.inner
                .deps
                .notify_acts(&code, Some(&venue.0), &text, acts)
                .await
        }
        // Решение 248: замер выхода продажи ниже модели — закрыть для входа; свежие налоги — снять, если сошлись.
        Touched::ExitGap(g) => super::exit_gap::on_gap(core, g).await,
        Touched::CoinCheck(c) => super::exit_gap::on_check(core, &c).await,
        _ => {}
    }
}
