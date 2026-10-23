//! Файл: venues/bsc_dex/check/quoter_cycle.rs
//! Что делает: цикл сверки локальной математики пулов с квотером своей DEX (каждые dex.quoter_check_s и по кнопке
//! «Перепроверить»): расхождение больше dex.quoter_max_diff_pct — пул исключён из котировок до следующей сверки.
//! Сверяются все пулы, и с крючком тоже: у пула с крючком B/C локальный выход берётся за вычетом измеренного
//! влияния крючка на покупку (check/hooks_probe.rs; сверка идёт на check.probe_usdt USDT — это покупка). Влияние ещё не
//! измерено или проверка крючка не прошла — сверка не пройдена, пул не используется (quote/select.rs).
//! Кто вызывает: pools/market.rs.
//! От чего зависит: quote/quoter.rs, check/state.rs (влияние крючка), pools/listen.rs (Pools), tokens.rs, num.rs.
//! Настройки: dex.quoter_check_s, dex.quoter_max_diff_pct, check.probe_usdt [bsc_dex].
//!
//! «Сверка: периодически и перед сделкой — квотер; расхождение выше порога → стоп по пулу» —
//! research/flow/bsc_dex.md, схема адаптера п. 6.

use std::sync::Arc;
use std::time::Duration;

use rust_decimal::Decimal;
use tokio::sync::Notify;

use crate::core::types::{CheckState, HookGroup};

use crate::storage::journal::Level;

use super::super::num;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::super::quote::quoter;
use super::state::Checks;

/// Цикл сверки с квотером всех достоверных пулов (см. шапку). Ошибок нет: сбой — причина в итоге пула.
pub async fn run(p: Pools, wake: Arc<Notify>) {
    loop {
        for r in p.reg.refs() {
            let Some(e) = p.reg.get(&r) else { continue };
            if !e.valid {
                continue;
            }
            let hook = hook_buy_pct(&p.checks, &e);
            let res = match hook {
                Err(why) => Err(why),
                Ok(pct) => match p
                    .tokens
                    .decimals(&p.ctx, e.usdt)
                    .await
                    .and_then(|d| p.ctx.dec("check.probe_usdt").map(|v| num::to_units(v, d)))
                {
                    Ok(Some(amount)) => quoter::compare(&p.ctx, &e, e.usdt, amount, pct)
                        .await
                        .unwrap_or_else(|err| Err(err.message)),
                    Ok(None) => Err("check.probe_usdt не переводится в единицы USDT".into()),
                    Err(err) => Err(err.message),
                },
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

/// Влияние крючка на покупку, %: 0 — крючка B/C нет. Err — влияние ещё не измерено или проверка крючка не прошла.
fn hook_buy_pct(checks: &Checks, e: &Entry) -> Result<Decimal, String> {
    if e.pool
        .hooks
        .as_ref()
        .is_none_or(|h| h.group == HookGroup::A)
    {
        return Ok(Decimal::ZERO);
    }
    match checks.pool(&e.pool.reference).hook.map(|c| c.impact) {
        Some(CheckState::Known(i)) => Ok(i.buy_pct),
        Some(CheckState::Failed { reason }) => Err(format!("проверка крючка не прошла: {reason}")),
        _ => Err("влияние крючка ещё не измерено — сверка с квотером невозможна".into()),
    }
}
