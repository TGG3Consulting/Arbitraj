//! Файл: venues/bsc_dex/quote/mixed_quoter.rs
//! Что делает: сверка умного маршрута перед отправкой — выход каждой доли (один пул или путь через промежуточную
//! монету: paths = токены по порядку, действие и параметры на каждый пул) по MixedQuoter PancakeSwap
//! (quoteMixedExactInput, eth_call на «latest») против локального расчёта доли. Квотер исполняет настоящий обмен
//! (с крючками) и откатывается внутри, поэтому локальный выход сравнивается с учётом влияния крючка.
//! Решение 261 (Тигран 12.11): сверка перед сделкой — односторонняя. Выход доли по квотеру МЕНЬШЕ нашего расчёта
//! больше чем на dex.quoter_max_diff_pct хотя бы у одной доли — сделки нет (как раньше); БОЛЬШЕ (на любую величину) —
//! в нашу сторону, сделка идёт, строка «в нашу сторону» — в хронологию сделки. Шаг Infinity Stable — ещё сверка с
//! get_dy крючка параллельно (quote/quoter_extra.rs, решение «122 (Infinity Stable, ответы)»), то же правило.
//! Минимум выхода — по нашему расчёту (exec/leg_min.rs, решение 228), квотер его не поднимает.
//! Кто вызывает: exec/smart_legs.rs (сделка), exec/dry_build.rs (сухая сделка).
//! От чего зависит: node/call.rs, pools/registry.rs, pools/key.rs, quote/quoter.rs (diff_pct, less_hook), quote/quoter_extra.rs.
//! Настройки: smart.mixed_quoter, dex.quoter_max_diff_pct [bsc_dex].
//!
//! Вызов quoteMixedExactInput (действия, параметры, ABI) — quote/mixed_quoter_calls.rs (вынесено решением 261: файл был
//! больше 200 строк; логика та же).

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::mixed_quoter_calls::request;
use super::path::Path;
use super::quoter::{diff_pct, less_hook};

/// Доля для сверки: путь (один пул или два через промежуточную монету).
pub struct Leg<'a> {
    pub path: &'a Path,
}

/// Сверка всех долей. Квотер исполняет весь путь доли (с крючками каждого пула); локальный выход последнего пула
/// берётся за вычетом влияния его крючка (влияние крючка первого пула уже учтено во входе второго — quote/via.rs).
/// Возвращает: строки «в нашу сторону» (решение 261; пусто — все в допуске). Ошибки: NotConfigured (адрес, допуск),
/// узел (откат квотера — его текст), QuoteMismatch (квотер меньше нашего больше допуска — с цифрами), Internal (у пула
/// V4 нет ключа, пустой путь).
pub async fn check(ctx: &Ctx, legs: &[Leg<'_>]) -> GatewayResult<Vec<String>> {
    let to = ctx.addr("smart.mixed_quoter")?;
    let limit = ctx.dec("dex.quoter_max_diff_pct")?;
    let calls = legs
        .iter()
        .map(|l| request(ctx, l.path))
        .collect::<GatewayResult<Vec<_>>>()?;
    // Решение «122 (Infinity Stable, ответы)»: шаги Infinity Stable — ещё get_dy крючка на свой вход (дополнительный
    // квотер, quote/quoter_extra.rs), параллельно с MixedQuoter (основной: действие INFI_CL — swap с крючком).
    let extras = legs.iter().flat_map(|l| l.path.hops.iter()).map(|h| {
        super::quoter_extra::check(
            ctx,
            &h.entry,
            h.token_in,
            h.swapped.amount_in,
            h.swapped.amount_out,
            ("latest", true),
        )
    });
    let (outs, extras) = tokio::join!(
        call::view_many(ctx, to, &calls, "latest"),
        futures_util::future::join_all(extras)
    );
    let mut favour = Vec::new();
    for r in extras {
        match r? {
            Ok(line) => favour.extend(line),
            Err(why) => return Err(ctx.err(ErrorCode::QuoteMismatch, why)),
        }
    }
    let outs = outs?;
    for (l, out) in legs.iter().zip(outs) {
        let last = l
            .path
            .hops
            .last()
            .ok_or_else(|| ctx.err(ErrorCode::Internal, "доля без пулов"))?;
        let local = less_hook(last.swapped.amount_out, last.usable.hook_pct).ok_or_else(|| {
            ctx.err(
                ErrorCode::QuoteMismatch,
                format!("влияние крючка {}% не считается", last.usable.hook_pct),
            )
        })?;
        match diff_pct(local, out.amountOut) {
            Some(d) if d <= limit => {}
            // Решение 261: выход сети больше нашего — в нашу сторону, сделка идёт.
            Some(d) if out.amountOut > local => favour.push(format!(
                "{} {}: расчёт {local}, MixedQuoter {} (+{}%) — в нашу сторону, сделка идёт (решение 261)",
                last.entry.family.title,
                last.entry.ticker,
                out.amountOut,
                d.round_dp(4)
            )),
            d => {
                return Err(ctx.err(
                    ErrorCode::QuoteMismatch,
                    format!(
                        "{} {}: расчёт {local} против MixedQuoter {} — расхождение {} > {limit}% (dex.quoter_max_diff_pct)",
                        last.entry.family.title,
                        last.entry.ticker,
                        out.amountOut,
                        d.map_or("не считается".into(), |d| format!("{d}%"))
                    ),
                ));
            }
        }
    }
    Ok(favour)
}
