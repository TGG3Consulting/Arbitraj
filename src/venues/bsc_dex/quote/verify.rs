//! Файл: venues/bsc_dex/quote/verify.rs
//! Что делает: предторговая верификация выбранного маршрута точечным Multicall3 (решение 83, фаза 80.6). Перед
//! отправкой перечитывает ТОЛЬКО пулы маршрута одним батчем на свежем блоке (pools/load_batch.rs states), применяет
//! свежее состояние к клонам записей реестра (pools/load.rs renewed — реестр не трогаем) и пересчитывает маршрут нашей
//! математикой (quote/verify_decide.rs: route_calc + costs). Вердикт: Ok{свежая котировка, amountOutMin по ногам,
//! блок} — данные свежие и цена не хуже границы сделки; Reject{причина, дрейф} — узел отдал устаревшее состояние (блок
//! отстал от головы больше verify.max_staleness_blocks), чтение/таймаут (verify.timeout_ms), свежая средняя цена хуже
//! границы сделки ядра (req.worst_price, решение 121 B4) или скачок цены больше грубого предохранителя
//! verify.max_price_drift_pct (в любую сторону). Порог прибыли — ядро (precheck/profit).
//! Reject → ордер не формируем (стартовое правило Тиграна) — врезку за правилом шлюза делает exec/verify_gate.rs (80.6b).
//! Решение 331: «одним батчем» верно только для V2/V3/CL/V4 (ступени Multicall3); пулы видов «по одному» (Algebra,
//! Slipstream, Bin, LB, DODO, стейблсвапы, WooFi и др., load_batch::one_by_one) читались друг за другом, и ступени
//! ждали их. При verify.read_parallel_max они читаются одновременно (не больше столько сразу) и одновременно со
//! ступенями — тот же блок, тот же итог (pools/load_batch_par.rs); не задана — как раньше.
//! Кто вызывает: exec/verify_gate.rs (предторговый путь DEX, графовый маршрут); тесты.
//! От чего зависит: pools/load_batch.rs, pools/load_batch_par.rs, pools/load.rs, pools/registry.rs,
//! quote/verify_decide.rs, quote/via.rs,
//! quote/steps.rs, node/heads.rs, tokens.rs, pools/fusion_book.rs (isolated — решение 463).
//! Настройки: verify.max_staleness_blocks, verify.max_price_drift_pct, verify.timeout_ms, exec.dex_slippage_pct,
//! verify.read_parallel_max (решение 331, необязательная) [bsc_dex].

use std::collections::HashMap;
use std::time::Duration;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{PoolRef, Quote};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::load_batch::Want;
use super::super::pools::load_batch_par;
use super::super::pools::registry::Entry;
use super::super::pools::{fusion_book, load};
use super::steps::Names;
use super::via::Via;

/// Настройка предела одновременных чтений пулов маршрута (решение 331); её же читает quote/cycle_legs.rs.
pub const PAR: &str = "verify.read_parallel_max";

// Пересчёт и вердикт по дрейфу (чистая часть) — отдельный файл, та же фича verify (решение 83).
pub use super::verify_decide::{Bounds, decide};

/// Итог верификации маршрута перед сделкой.
pub enum Verdict {
    /// Данные свежие и цена в допуске: свежая котировка, amountOutMin по каждой ноге (решение 84), блок перечтения,
    /// свежие записи пулов маршрута на этом блоке (решение «335 (В1, пакет)»: проба крючков перед сделкой берёт их, а
    /// не читает пулы второй раз — check/hook_batch.rs).
    Ok {
        fresh_quote: Box<Quote>,
        amount_out_min: Vec<Decimal>,
        block: u64,
        pools: std::sync::Arc<HashMap<PoolRef, Entry>>,
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
/// цену газа, границу сделки ядра (req.worst_price — решение 121 B4). Возвращает: вердикт. Ошибки: NotConfigured (verify.*, exec.dex_slippage_pct), NoRoute (нет маршрута),
/// узел/настройки чтения.
pub async fn run(
    v: &Via<'_>,
    names: &Names<'_>,
    plan: &Quote,
    gas: U256,
    worst: Decimal,
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
            token0: e.pool.token0,
            token1: e.pool.token1,
        })
        .collect();
    // Пулы маршрута на свежем блоке (решение 83), с таймаутом: V2/V3/CL/V4 — ступенями Multicall3; виды «по одному» —
    // по очереди (не задана verify.read_parallel_max) или одновременно с пределом (решение 331, load_batch_par.rs).
    let par = load_batch_par::limit(ctx, PAR);
    // Решение 463 (повторное ревью t236): оракул THENA Fusion этого чтения — не в книгу (fusion_book.rs isolated).
    let fetch = load_batch_par::states_with(ctx, &v.q.tokens, &v.q.reg, &wants, block, par);
    let fetch = fusion_book::isolated(fetch);
    let limit = Duration::from_millis(timeout_ms);
    let started = tokio::time::Instant::now();
    let loaded = match tokio::time::timeout(limit, fetch).await {
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
    let measured = if let [leg] = route.legs.as_slice()
        && let Some(order) = super::super::exec::yield_basis_requote::selected(&leg.hops)
    {
        let entry = &fresh[&leg.hops[0].pool];
        let measured = Box::pin(super::super::exec::yield_basis_requote::path(
            v,
            entry,
            plan.qty,
            route.mode == crate::core::types::RouteMode::Direct,
            order,
            Some(block),
        ));
        let path =
            match tokio::time::timeout(limit.saturating_sub(started.elapsed()), measured).await {
                Ok(Ok(value)) => value,
                Ok(Err(err)) => {
                    return Ok(Verdict::Reject {
                        reason: err.message,
                        drift: None,
                    });
                }
                Err(_) => {
                    return Ok(Verdict::Reject {
                        reason: "Проверка выплаты кошельку не уложилась в verify.timeout_ms".into(),
                        drift: None,
                    });
                }
            };
        Some(super::costs::quote(
            ctx,
            &plan.asset,
            v.side,
            plan.qty,
            &[path],
            route.mode,
            v.taxes,
            gas,
            names,
        )?)
    } else {
        None
    };
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
    let bounds = Bounds {
        max_drift,
        slip_pct,
        worst,
    };
    Ok(match measured {
        Some(quote) => super::verify_decide::judge(plan, quote, v.side, &fresh, block, bounds),
        None => decide(v, names, plan, &fresh, gas, block, bounds),
    })
}
