//! Файл: venues/bsc_dex/check/cycle.rs
//! Что делает: циклы проверок (решения 30, 32): налоги всех монет (tax.recheck_interval_s), влияние крючков пулов
//! V4 группы B/C (hooks.recheck_interval_s), сверка локальной математики с квотером (dex.quoter_check_s).
//! Первый проход — сразу при подписке, до него монеты «не проверены» (событие CoinCheck NotChecked — торговли нет).
//! Кнопка «Перепроверить» (recheck_coins) будит все три цикла сразу. Итоги — события CoinCheck / PoolCheck ядру и журнал.
//! Сверка с квотером — check/quoter_cycle.rs.
//! Кто вызывает: pools/market.rs (запуск при подписке), mod.rs (recheck_coins).
//! От чего зависит: check/taxes.rs, check/hooks_probe.rs, quote/quoter.rs, check/state.rs, pools/listen.rs (Pools).
//! Настройки: tax.recheck_interval_s, hooks.recheck_interval_s (вся система), dex.quoter_check_s, check.probe_usdt [bsc_dex].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::Address;
use tokio::sync::Notify;

use crate::core::types::{Asset, CheckState, CoinCheck};
use crate::gateway::events::MarketEvent;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::{hooks_probe, taxes};

/// Цикл налогов. Принимает: окружение, монеты (адрес → тикер), USDT, сигнал «перепроверить». Работает постоянно.
pub async fn taxes(p: Pools, coins: HashMap<Address, String>, usdt: Address, wake: Arc<Notify>) {
    for (addr, ticker) in &coins {
        let c = CoinCheck {
            venue: p.ctx.venue(),
            asset: Asset {
                ticker: ticker.clone(),
                bsc_contract: Some(*addr),
            },
            taxes: CheckState::NotChecked,
            source: taxes::SOURCE.into(),
            checked_at: None,
        };
        p.checks.set_coin(c.clone());
        let _ = p.out.try_send(MarketEvent::CoinCheck(c));
    }
    loop {
        let (mut ok, mut failed) = (0, 0);
        for (addr, ticker) in &coins {
            let block_time = match p.heads.fresh(&p.ctx) {
                Ok(h) => h.timestamp,
                Err(e) => {
                    p.ctx
                        .log(
                            Level::Warn,
                            "dex.check",
                            format!("Проверка налогов отложена: {}", e.message),
                            None,
                        )
                        .await;
                    break;
                }
            };
            let c = taxes::check_coin(
                &p.ctx, &p.reg, &p.checks, &p.tokens, ticker, *addr, usdt, block_time,
            )
            .await;
            match &c.taxes {
                CheckState::Known(_) => ok += 1,
                _ => failed += 1,
            }
            if let CheckState::Failed { reason } = &c.taxes {
                p.ctx
                    .log(
                        Level::Warn,
                        "dex.check",
                        format!("{ticker}: проверка налогов не прошла: {reason}"),
                        None,
                    )
                    .await;
            }
            p.checks.set_coin(c.clone());
            if p.out.try_send(MarketEvent::CoinCheck(c)).is_err() && p.out.is_closed() {
                return;
            }
        }
        p.ctx
            .log(
                Level::Info,
                "dex.check",
                format!("Проверка налогов: прошло {ok}, не прошло {failed}"),
                None,
            )
            .await;
        if !sleep_or_wake(&p, "tax.recheck_interval_s", &wake).await {
            return;
        }
    }
}

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
async fn sleep_or_wake(p: &Pools, setting: &str, wake: &Notify) -> bool {
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
