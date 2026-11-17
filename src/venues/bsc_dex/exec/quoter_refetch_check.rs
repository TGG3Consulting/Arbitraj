//! Файл: venues/bsc_dex/exec/quoter_refetch_check.rs
//! Что делает: решение 382 — сверки с квотером на блоке перечитанных пулов (Quoting.pinned, exec/quoter_refetch.rs):
//! хопы пути квотером пула (check_on — как exec/exit_quote.rs check_with), умный маршрут MixedQuoter (mixed —
//! quote/mixed_quoter.rs check_at), хопы Maverick исполнителя и сухой сборки (через exec/precheck_quoter.rs с блоком).
//! Без перечитанных пулов — «latest», как раньше. plan_gap (ревью Н1) — цена по перечитанным пулам не хуже плана
//! больше допуска.
//! Кто вызывает: exec/trade_direct_prep.rs, exec/smart_legs.rs, exec/dry_checks.rs, exec/precheck_quoter.rs.
//! От чего зависит: exec/precheck.rs (quoter_hop_at), exec/exit_quote.rs, quote/mixed_quoter.rs, node/call.rs.
//! Настройки: через вызываемые — dex.quoter [вид пула], smart.mixed_quoter, dex.quoter_max_diff_pct [bsc_dex].

use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::quote::mixed_quoter::{self, Leg};
use super::super::quote::path::{Hop, Path};
use super::exit_quote::ExitFix;
use super::precheck;

/// Тег блока для eth_call: номер перечитанных пулов (hex) или «latest». Ошибок нет.
pub fn tag(block: Option<u64>) -> String {
    block.map_or_else(|| "latest".to_string(), call::block_tag)
}

/// Сверка с MixedQuoter (решение 261, односторонняя) на блоке перечитанных пулов или «latest». Принимает: окружение,
/// доли, блок. Возвращает и ошибки: как quote/mixed_quoter.rs check.
pub async fn mixed(ctx: &Ctx, legs: &[Leg<'_>], block: Option<u64>) -> GatewayResult<Vec<String>> {
    mixed_quoter::check_at(ctx, legs, &tag(block)).await
}

/// Ревью Н1: план ядра посчитан по памяти, которая разошлась с квотером. По перечитанным пулам средняя цена доли(ей)
/// не должна быть хуже средней цены плана больше чем на dex.quoter_max_diff_pct: иначе — QuoteMismatch «план по
/// неверной памяти» (ядро пересчитает план с проверкой прибыли), а не сделка на границе exec.dex_slippage_pct.
/// Средняя цена плана — из худшей допустимой (обратная формула core/worst.rs, как exec/verify_gate.rs plan_quote).
/// Продажа выхода (решение 177) не проверяется — она продаёт по квотеру. Принимает: окружение, запрос, монет и USDT
/// по перечитанным пулам. Ошибки: QuoteMismatch, NotConfigured (exec.dex_slippage_pct, dex.quoter_max_diff_pct).
pub fn plan_gap(ctx: &Ctx, req: &TradeRequest, coins: Decimal, usdt: Decimal) -> GatewayResult<()> {
    if super::exit_quote::exit_of(req) || coins.is_zero() {
        return Ok(());
    }
    let slip = ctx.dec("exec.dex_slippage_pct")? / Decimal::ONE_HUNDRED;
    let limit = ctx.dec("dex.quoter_max_diff_pct")?;
    let denom = match req.side {
        Side::Buy => Decimal::ONE + slip,
        Side::Sell => Decimal::ONE - slip,
    };
    if denom <= Decimal::ZERO {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            "exec.dex_slippage_pct ≥ 100% — средняя цена плана не восстанавливается",
        ));
    }
    let plan = req.worst_price / denom;
    if plan.is_zero() {
        return Ok(());
    }
    let now = usdt / coins;
    // Хуже для нас: покупка — дороже плана, продажа — дешевле.
    let worse = match req.side {
        Side::Buy => (now - plan) / plan * Decimal::ONE_HUNDRED,
        Side::Sell => (plan - now) / plan * Decimal::ONE_HUNDRED,
    };
    if worse > limit {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            format!(
                "план по неверной памяти: по пулам, перечитанным с узла, средняя цена {} хуже плана {} на {}% > {limit}% \
                 (допуск dex.quoter_max_diff_pct) — сделки нет (решение 382)",
                now.round_dp(10),
                plan.round_dp(10),
                worse.round_dp(4)
            ),
        ));
    }
    Ok(())
}

/// Сверка хопов пути с квотером на блоке `block` (пулы пути перечитаны на нём) — как exec/exit_quote.rs check_with,
/// но без известного выхода пробы: квотер каждого хопа вызывается на этом блоке (precheck::quoter_hop_at). Принимает:
/// окружение, путь, (продажа ли выхода, какие хопы сверять), блок. Возвращает: расхождения (номер хопа, цифры).
/// Ошибки: как exit_quote::check_with; StaleData — состояния блока у узла нет.
pub async fn check_on(
    ctx: &Ctx,
    path: &Path,
    (exit, pick): (bool, impl Fn(&Hop) -> bool),
    block: u64,
) -> GatewayResult<Vec<(usize, ExitFix)>> {
    let mut out = Vec::new();
    for (i, h) in path.hops.iter().enumerate().filter(|(_, h)| pick(h)) {
        let fix = precheck::quoter_hop_at(
            ctx,
            &h.entry,
            h.token_in,
            &h.swapped,
            (h.usable.hook_pct, exit),
            block,
        )
        .await?;
        if let Some(f) = fix {
            out.push((i, f));
        }
    }
    Ok(out)
}
