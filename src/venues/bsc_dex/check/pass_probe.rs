//! Файл: venues/bsc_dex/check/pass_probe.rs
//! Что делает: решение 401 (t186) — проба шага пула менеджера (Uniswap V4, Infinity CL / Bin / Stable) ЧЕРЕЗ НАСТОЯЩИЙ
//! исполнитель v3 кошелька (адрес из «Маппинга кошельков», код не подставляется: крючок с белым списком смотрит
//! msg.sender, и проба на чужом адресе ничего не доказывает). eth_simulateV1 от кошелька поверх головы H (время + 1 с,
//! check/sim_base.rs), две симуляции, вторая — продолжение первой:
//! 1. перевод встречного токена пула (USDT или промежуточная монета, решение 52) на кошелёк из другого пула реестра
//!    (как проба налогов, check/taxes_util.rs quote_source) на сумму check.probe_usdt (в промежуточной — quote/rate.rs);
//!    разрешение исполнителю (wallet/approve_executor.rs); квотер DEX на покупку (quote/probe_quote.rs — тот же, что у
//!    пробы налогов); execute одним шагом пула (встречный → монета, выход — на кошелёк); balanceOf(исполнитель) по
//!    обоим токенам;
//! 2. то же + разрешение монеты; квотер на продажу купленного (в состоянии после покупки); execute продажи; остатки.
//!
//! Итог — check/pass_judge.rs (прошёл обе стороны / отвергнут с текстом отката / не проверено). До пробы: налоги
//! монеты в ЭТОМ пуле (PoolFacts.taxes, проба налогов Universal Router) — только нули; налог или «не измерены» — «не
//! проверено» (через исполнитель налоговая монета пулом менеджера не идёт: вход переводом через исполнитель — налог
//! дважды, quote/graph_executor.rs tax; выход с налогом с квотером не сверить).
//! Кто вызывает: check/pass_cycle.rs.
//! От чего зависит: check/{pass_calls, pass_judge, simulate, sim_base, probe_calls, taxes_util, state}.rs,
//! quote/{probe_quote, rate, select}.rs, wallet/signer.rs, pools/listen.rs (Pools); вызовы пробы — check/pass_calls.rs.
//! Настройки: check.probe_usdt, dex.quoter_max_diff_pct, dex.deadline_s [bsc_dex]; через вызываемые — адрес квотера
//! [DEX], dex.vault [DEX Infinity], dex.quoter_virtual [DEX].

use alloy_primitives::Address;

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::exec_kind::ExecKind;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::super::quote::probe_quote::Way;
use super::super::quote::{rate, select};
use super::super::wallet::signer;
use super::pass_calls::{approvals, call_of, prep_failed, side_of, tail};
use super::pass_judge::{self, Outcome};
use super::probe_calls as pc;
use super::sim_base;
use super::simulate;
use super::state::PoolTaxes;
use super::taxes_util::{Sources, quote_source};

/// Проба пула через исполнитель кошелька. Принимает: состояние пулов, окружение кошелька (счёт с исполнителем v3),
/// источники встречного токена прохода, пул, (адрес исполнителя, вид шага, крючок ключа).
/// Возвращает: итог и блок пробы (None — до узла не дошла). Ошибки: настройки, ключ кошелька, узел, нет пула-источника,
/// откат подготовки (NoRoute) — вызывающий оставляет прежний факт (нет прежнего — «не проверено» с этой причиной).
pub async fn run(
    p: &Pools,
    ctx: &Ctx,
    sources: &Sources,
    e: &Entry,
    (x, k, hooks): (Address, ExecKind, Address),
) -> GatewayResult<(Outcome, Option<u64>)> {
    if let Some(why) = taxed(p.checks.pool(&e.pool.reference).taxes.as_ref()) {
        return Ok((Outcome::Unchecked(why), None));
    }
    let max = ctx.dec("dex.quoter_max_diff_pct")?;
    let wallet = signer::address(ctx)?;
    let usdt = p.tokens.usdt(ctx)?;
    let usdt_dec = p.tokens.decimals(ctx, usdt).await?;
    let probe = num::to_units(ctx.dec("check.probe_usdt")?, usdt_dec).ok_or_else(|| {
        ctx.not_set(
            "check.probe_usdt",
            "check.probe_usdt не переводится в единицы USDT",
        )
    })?;
    let amount = rate::from_usdt(ctx, &p.reg, usdt, e.usdt, probe)?;
    let exclude = match &e.pool.reference {
        PoolRef::Contract(a) => Some(*a),
        _ => None,
    };
    let deep = |s: &Entry| {
        let any = select::Need::Any;
        matches!(
            select::deep_side(ctx, &p.reg, &p.tokens, s, usdt_dec, any),
            Ok(None)
        )
    };
    let found = quote_source(ctx, &p.reg, sources, e.usdt, exclude, amount, deep).await?;
    let Some(source) = found else {
        let why = format!("нет другого пула с {amount} единиц {:#x} для пробы", e.usdt);
        return Err(ctx.err(ErrorCode::NoRoute, why));
    };
    let (base, now) = sim_base::head(ctx, &p.heads).await?;
    let way = Way::of(ctx, e);
    let over = way.overrides(ctx)?;
    // 1. Покупка.
    let mut list = vec![pc::transfer(source, e.usdt, wallet, amount)];
    approvals(ctx, &mut list, (wallet, x), (e.usdt, amount))?;
    let buy_call = call_of(ctx, e, k, (e.usdt, e.coin, amount), x, now)?;
    let b = tail(ctx, &mut list, e, way, (wallet, x), &buy_call)?;
    let first = simulate::run_with(ctx, &list, &[], &over, base).await?;
    if let Some(err) = prep_failed(ctx, &first[..b.quote]) {
        return Err(err);
    }
    let buy_first = side_of(ctx, e, way, &first, b, ("покупка", e.usdt, amount));
    let got = match pass_judge::side(&buy_first, hooks, max) {
        Ok(g) => g,
        Err(o) => return Ok((o, Some(base.number))),
    };
    // 2. Продажа купленного — продолжение первой симуляции.
    let start = list.len();
    approvals(ctx, &mut list, (wallet, x), (e.coin, got))?;
    let sell_call = call_of(ctx, e, k, (e.coin, e.usdt, got), x, now)?;
    let s = tail(ctx, &mut list, e, way, (wallet, x), &sell_call)?;
    let all = simulate::run_with(ctx, &list, &[], &over, base).await?;
    if let Some(err) = prep_failed(ctx, &all[start..s.quote]) {
        return Err(err);
    }
    let buy_again = side_of(ctx, e, way, &all, b, ("покупка", e.usdt, amount));
    let sell = side_of(ctx, e, way, &all, s, ("продажа", e.coin, got));
    let o = pass_judge::judge(&buy_again, Some(&sell), hooks, max);
    Ok((o, Some(base.number)))
}

/// Причина «налоги пула не измерены».
pub const UNMEASURED: &str =
    "налоги монеты в этом пуле ещё не измерены (проба налогов) — проба через исполнитель ждёт их";

/// Налоги монеты в пуле годятся для пробы через исполнитель: измерены и все нули. None — годятся, иначе причина.
/// Ошибок нет.
pub fn taxed(t: Option<&PoolTaxes>) -> Option<String> {
    let Some(t) = t else {
        return Some(UNMEASURED.into());
    };
    let zero =
        t.buy_pct.is_zero() && t.sell_pct.is_some_and(|s| s.is_zero()) && t.transfer_pct.is_zero();
    if zero {
        return None;
    }
    let sell = t
        .sell_pct
        .map_or("не измерен".to_string(), |s| format!("{s} %"));
    Some(format!(
        "монета с налогом в этом пуле (покупка {} %, продажа {sell}, перевод {} %) — через исполнитель не проверяется, \
         пул идёт прежним путём",
        t.buy_pct.normalize(),
        t.transfer_pct.normalize()
    ))
}
