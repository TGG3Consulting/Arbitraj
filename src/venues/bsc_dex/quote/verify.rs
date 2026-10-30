//! Файл: venues/bsc_dex/quote/verify.rs
//! Что делает: предторговая верификация выбранного маршрута точечным Multicall3 (решение 83, фаза 80.6). Перед
//! отправкой перечитывает ТОЛЬКО пулы маршрута одним батчем на свежем блоке (pools/load_batch.rs states), применяет
//! свежее состояние к клонам записей реестра (pools/load.rs renewed — реестр не трогаем) и пересчитывает маршрут нашей
//! математикой (quote/verify_decide.rs: route_calc + costs). Вердикт: Ok{свежая котировка, amountOutMin по ногам,
//! блок} — данные свежие и цена не уехала; Reject{причина, дрейф} — узел отдал устаревшее состояние (блок отстал от
//! головы больше verify.max_staleness_blocks), чтение/таймаут (verify.timeout_ms), или цена уехала больше
//! verify.max_price_drift_pct. Verify проверяет ТОЛЬКО валидность данных; порог прибыли — ядро (precheck/profit).
//! Reject → ордер не формируем (стартовое правило Тиграна) — врезку за правилом шлюза делает exec/verify_gate.rs (80.6b).
//! Кто вызывает: exec/verify_gate.rs (предторговый путь DEX, графовый маршрут); тесты.
//! От чего зависит: pools/load_batch.rs, pools/load.rs, pools/registry.rs, quote/verify_decide.rs, quote/via.rs,
//! quote/steps.rs, node/heads.rs, tokens.rs.
//! Настройки: verify.max_staleness_blocks, verify.max_price_drift_pct, verify.timeout_ms, exec.dex_slippage_pct [bsc_dex].

use std::collections::HashMap;
use std::time::Duration;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{PoolRef, Quote};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::load;
use super::super::pools::load_batch::{self, Want};
use super::super::pools::registry::Entry;
use super::steps::Names;
use super::via::Via;

// Пересчёт и вердикт по дрейфу (чистая часть) — отдельный файл, та же фича verify (решение 83).
pub use super::verify_decide::decide;

/// Итог верификации маршрута перед сделкой.
pub enum Verdict {
    /// Данные свежие и цена в допуске: свежая котировка, amountOutMin по каждой ноге (решение 84), блок перечтения.
    Ok {
        fresh_quote: Box<Quote>,
        amount_out_min: Vec<Decimal>,
        block: u64,
    },
    /// Не исполняем: устаревшее состояние, сбой/таймаут чтения или цена уехала. drift — дрейф (None, если до него не дошло).
    Reject {
        reason: String,
        drift: Option<Decimal>,
    },
}

/// Пределы верификации из настроек. Ошибки: NotConfigured — не задано (verify не работает).
pub fn limits(ctx: &super::super::ctx::Ctx) -> GatewayResult<(u64, Decimal, u64)> {
    let stale = ctx.int("verify.max_staleness_blocks")?;
    let drift = ctx.dec("verify.max_price_drift_pct")?;
    let timeout = ctx.int("verify.timeout_ms")?;
    if timeout == 0 {
        return Err(ctx.not_set(
            "verify.timeout_ms",
            "verify.timeout_ms должен быть больше нуля",
        ));
    }
    Ok((stale, drift, timeout))
}

/// Верификация маршрута котировки `plan`. Принимает: расчёт путей (сторона, монета, налоги, decimals), тикеры, план,
/// цену газа. Возвращает: вердикт. Ошибки: NotConfigured (verify.*, exec.dex_slippage_pct), NoRoute (нет маршрута),
/// узел/настройки чтения.
pub async fn run(
    v: &Via<'_>,
    names: &Names<'_>,
    plan: &Quote,
    gas: U256,
) -> GatewayResult<Verdict> {
    let ctx = &v.q.ctx;
    let (max_stale, max_drift, timeout_ms) = limits(ctx)?;
    // Допуск проскальзывания для amountOutMin (решение 84) — через ?, как боевой путь (order.rs): не задан → NotConfigured,
    // верификация не работает (не «тихий ноль»).
    let slip_pct = ctx.dec("exec.dex_slippage_pct")?;
    let route = plan
        .route
        .as_ref()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "нет маршрута для верификации"))?;
    // Пулы маршрута (уникальные), их записи из реестра — для Want и восстановления ног.
    let mut seen = HashMap::new();
    let refs: Vec<PoolRef> = route
        .legs
        .iter()
        .flat_map(|l| &l.hops)
        .map(|h| h.pool.clone())
        .filter(|r| seen.insert(r.clone(), ()).is_none())
        .collect();
    let entries: Vec<Entry> = refs
        .iter()
        .map(|r| {
            v.q.reg
                .get(r)
                .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "пул маршрута больше не в реестре"))
        })
        .collect::<GatewayResult<_>>()?;
    let block =
        v.q.heads
            .latest()
            .ok_or_else(|| ctx.err(ErrorCode::StaleData, "нет свежего блока для верификации"))?
            .number;
    let wants: Vec<Want> = entries
        .iter()
        .map(|e| Want {
            family: e.family,
            reference: &e.pool.reference,
            spacing: e.pool.tick_spacing,
            step: e.pool.bin_step,
            usdt: e.usdt,
            token1: e.pool.token1,
        })
        .collect();
    // Один целевой Multicall3-батч по пулам маршрута на свежем блоке (решение 83), с таймаутом.
    let fetch = load_batch::states(ctx, &v.q.tokens, &wants, block);
    let loaded = match tokio::time::timeout(Duration::from_millis(timeout_ms), fetch).await {
        Ok(v) => v,
        Err(_) => {
            return Ok(Verdict::Reject {
                reason: format!(
                    "перечтение маршрута не уложилось в verify.timeout_ms ({timeout_ms} мс)"
                ),
                drift: None,
            });
        }
    };
    // Свежее состояние — на клоны записей (реестр не трогаем).
    let mut fresh: HashMap<PoolRef, Entry> = HashMap::new();
    for (e, l) in entries.iter().zip(loaded) {
        match l {
            Ok(st) => {
                fresh.insert(e.pool.reference.clone(), load::renewed(e, block, st));
            }
            Err(err) => {
                return Ok(Verdict::Reject {
                    reason: format!("пул маршрута не перечитан: {}", err.message),
                    drift: None,
                });
            }
        }
    }
    // Устаревшее состояние: прочитанный блок отстал от головы больше verify.max_staleness_blocks.
    let head = v.q.heads.latest().map_or(block, |h| h.number);
    if head.saturating_sub(block) > max_stale {
        return Ok(Verdict::Reject {
            reason: format!(
                "состояние устарело: блок {block} отстал от головы {head} больше verify.max_staleness_blocks ({max_stale})"
            ),
            drift: None,
        });
    }
    Ok(decide(
        v, names, plan, &fresh, gas, block, max_drift, slip_pct,
    ))
}
