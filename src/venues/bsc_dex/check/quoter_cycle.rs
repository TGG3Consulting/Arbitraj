//! Файл: venues/bsc_dex/check/quoter_cycle.rs
//! Что делает: цикл сверки локальной математики пулов с квотером своей DEX (каждые dex.quoter_check_s и по кнопке
//! «Перепроверить»): расхождение больше dex.quoter_max_diff_pct — пул исключён из котировок до следующей сверки.
//! Пулы с крючком группы B/C не сверяются: их отличие от математики — это влияние крючка (check/hooks_probe.rs).
//! Кто вызывает: pools/market.rs.
//! От чего зависит: quote/quoter.rs, check/state.rs, pools/listen.rs (Pools), tokens.rs, num.rs.
//! Настройки: dex.quoter_check_s, dex.quoter_max_diff_pct, check.probe_usdt [bsc_dex].
//!
//! «Сверка: периодически и перед сделкой — квотер; расхождение выше порога → стоп по пулу» —
//! research/flow/bsc_dex.md, схема адаптера п. 6.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::storage::journal::Level;

use super::super::num;
use super::super::pools::listen::Pools;
use super::super::quote::quoter;

/// Цикл сверки с квотером (пулы без крючка группы B/C: у них отличие от математики — это и есть влияние крючка).
pub async fn run(p: Pools, wake: Arc<Notify>) {
    loop {
        for r in p.reg.refs() {
            let Some(e) = p.reg.get(&r) else { continue };
            let hooked = e
                .pool
                .hooks
                .as_ref()
                .is_some_and(|h| h.group != crate::core::types::HookGroup::A);
            if !e.valid || hooked {
                continue;
            }
            let res = match p
                .tokens
                .decimals(&p.ctx, e.usdt)
                .await
                .and_then(|d| p.ctx.dec("check.probe_usdt").map(|v| num::to_units(v, d)))
            {
                Ok(Some(amount)) => quoter::compare(&p.ctx, &e, e.usdt, amount)
                    .await
                    .unwrap_or_else(|err| Err(err.message)),
                Ok(None) => Err("check.probe_usdt не переводится в единицы USDT".into()),
                Err(err) => Err(err.message),
            };
            if let Err(why) = &res {
                p.ctx
                    .log(
                        Level::Warn,
                        "dex.quoter",
                        format!("{} {}: {why} — пул исключён", e.family.title, e.ticker),
                        None,
                    )
                    .await;
            }
            p.checks.update_pool(&r, |f| f.quoter = Some(res));
        }
        let wait = p.ctx.int("dex.quoter_check_s").map(Duration::from_secs);
        match wait {
            Ok(d) => tokio::select! { _ = tokio::time::sleep(d) => {}, _ = wake.notified() => {} },
            Err(e) => {
                p.ctx
                    .log(
                        Level::Error,
                        "dex.quoter",
                        format!("Сверка с квотером остановлена: {}", e.message),
                        None,
                    )
                    .await;
                return;
            }
        }
    }
}
