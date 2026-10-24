//! Файл: venues/bsc_dex/check/cycle.rs
//! Что делает: цикл проверок крючков (решение 32): влияние крючков пулов V4 группы B/C (hooks.recheck_interval_s).
//! Налоги — check/tax_cycle.rs, сверка с квотером — check/quoter_cycle.rs.
//! Кнопка «Перепроверить» (recheck_coins) будит все три цикла сразу. Итоги — события CoinCheck / PoolCheck ядру и журнал.
//! Сверка с квотером — check/quoter_cycle.rs.
//! Кто вызывает: pools/market.rs (запуск при подписке), mod.rs (recheck_coins).
//! От чего зависит: check/taxes.rs, check/hooks_probe.rs, quote/quoter.rs, check/state.rs, pools/listen.rs (Pools).
//! Настройки: tax.recheck_interval_s, hooks.recheck_interval_s (вся система), dex.quoter_check_s, check.probe_usdt [bsc_dex].

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::core::types::CheckState;
use crate::gateway::events::MarketEvent;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::hooks_probe;

/// Цикл крючков V4 группы B/C. Работает постоянно.
pub async fn hooks(p: Pools, wake: Arc<Notify>) {
    loop {
        for r in p.reg.refs() {
            let Some(e) = p.reg.get(&r) else { continue };
            if !e.valid {
                continue;
            }
            if let Some(pc) = hooks_probe::check_pool(&p.ctx, &p.tokens, &p.reg, &e).await {
                if let CheckState::Failed { reason } = &pc.impact {
                    p.ctx
                        .log(
                            Level::Warn,
                            "dex.check",
                            format!(
                                "{} {}: крючок не проверен: {reason}",
                                e.family.title, e.ticker
                            ),
                            None,
                        )
                        .await;
                }
                p.checks.update_pool(&r, |f| f.hook = Some(pc.clone()));
                if p.out.try_send(MarketEvent::PoolCheck(pc)).is_err() && p.out.is_closed() {
                    return;
                }
            }
        }
        if !sleep_or_wake(&p, "hooks.recheck_interval_s", &wake).await {
            return;
        }
    }
}

/// Пауза по настройке системы или до сигнала «перепроверить». Возвращает false — настройка не задана (цикл остановлен).
pub async fn sleep_or_wake(p: &Pools, setting: &str, wake: &Notify) -> bool {
    match p.ctx.settings.integer(setting, At::default()) {
        Ok(s) => {
            tokio::select! { _ = tokio::time::sleep(Duration::from_secs(s)) => {}, _ = wake.notified() => {} }
            true
        }
        Err(e) => {
            p.ctx
                .log(
                    Level::Error,
                    "dex.check",
                    format!("Цикл проверок остановлен: {}", e.message),
                    None,
                )
                .await;
            false
        }
    }
}
