//! Файл: venues/bsc_dex/exec/ur_remeasure.rs
//! Что делает: решение 248 (техлид 10.11 по поручению Тиграна, кейс SMARS 10.11, сделка 20261110050248465SMARS:
//! «[свой сборщик: прямой] проба сделки перед отправкой: шаг 0 откатился — код 3: execution reverted: 0x849eaf98» на
//! каждом повторе до продажи руками) — пере-замер решения 247 и для прямого и умного пути (Universal Router).
//! 1) check: проба плана (exec/precheck_sim.rs run) откатилась «мало на выходе» (exec/ur_short.rs: V2/V3/Stable/
//!    Infinity/V4 TooLittleReceived, ExecutionFailed(BalanceTooLow)) у продажи выхода — монета во внеочередную
//!    перепроверку налогов (как 247 п. Б), вызывающий получает исходную ошибку для пере-замера; иначе — как раньше.
//! 2) measure: та же продажа (тот же кошелёк, количество, маршрут), собранная заново на текущем блоке с минимумом
//!    1 ед. у каждого шага (0 запрещён: exec/plan.rs) и проверкой баланса «база + сумма этих минимумов», симулируется
//!    от кошелька сделки поверх живого блока — с реального баланса кошелька, без пополнения от пула (exec/dry_fund.rs
//!    здесь не участвует); реальный выход R = баланс выходного токена кошелька после execute − до него (два balanceOf
//!    в том же симулированном блоке). Граница = min(R, выход при налоге, наблюдённом в сети) — та же, что у
//!    исполнителя (exec/executor/remeasure.rs bound_text); граница уходит ядру (exec/exit_gap.rs — решение 248 п. 2).
//!    Условия ядра есть (TradeRequest.remeasure) и граница ≥ floor — план отправки: минимумы шагов 1 ед., проверка
//!    баланса BALANCE_CHECK_ERC20 = база + граница × (1 − exec.dex_slippage_pct) (решение 40: проверка реально
//!    полученного в той же транзакции — защита от сэндвича по всему пути; промежуточные минимумы её не усиливают, а
//!    с налогом выше модели откатывают шаг 0, как у SMARS). Ниже floor или условий нет — отказ с числами.
//!    Повторная проба не нужна: тот же план, тот же блок, отличается только граница проверки баланса, M ≤ R.
//!
//! Кто вызывает: exec/trade_direct.rs, exec/trade_smart.rs.
//! От чего зависит: exec/{precheck_sim, ur_short, exit_gap, plan, chain, txs}.rs, exec/executor/remeasure.rs (граница,
//! minOut), check/{emitter, probe_calls, sim_base, recheck}.rs, num.rs, ctx.rs.
//! Настройки: exec.dex_slippage_pct [bsc_dex].

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::super::check::emitter;
use super::super::check::probe_calls as pc;
use super::super::check::sim_base::Base;
use super::super::num;
use super::super::pools::registry::Entry;
use super::chain::Step;
use super::executor::remeasure::{bound_text, min_out_of};
use super::plan::Plan;
use super::txs::Env;
use super::{exit_gap, precheck_sim, ur_short};

/// Проба плана перед отправкой. Возвращает: None — проба прошла (как precheck_sim::emitters); Some(ошибка) — продажа
/// выхода откатилась «мало на выходе»: вызывающий собирает план с минимумами 1 ед. и зовёт measure. Ошибки: как
/// precheck_sim::emitters (иной откат, срок, чужой пул, узел).
pub async fn check(
    env: &Env,
    req: &TradeRequest,
    p: &Plan,
    entries: &[&Entry],
    at: Base,
) -> GatewayResult<Option<GatewayError>> {
    let ctx = &env.ctx;
    let res = precheck_sim::run(ctx, p, at, None).await?;
    let short = res.iter().find(|r| !r.ok()).and_then(|r| {
        let text = r
            .error
            .as_ref()
            .map(|e| e.message.as_str())
            .unwrap_or_default();
        ur_short::too_little(&r.return_data, text)
    });
    let exit = req.side == Side::Sell && req.exit_position;
    let Some(name) = short.filter(|_| exit) else {
        return precheck_sim::judge(ctx, &res, entries).await.map(|_| None);
    };
    let Err(mut orig) = precheck_sim::judge(ctx, &res, entries).await else {
        return Ok(None);
    };
    orig.message = format!(
        "{} (Universal Router {name} — мало на выходе)",
        orig.message
    );
    // Решение 247, п. Б (и 248): модель налога монеты не сошлась с сетью — внеочередная перепроверка.
    if let Some(a) = req.asset.bsc_contract {
        env.wake.coins(&[a]);
        let msg = format!(
            "{} {}: откат продажи выхода «мало на выходе» ({name}) — монета в перепроверку налогов (решения 247 п. Б, 248)",
            req.deal_id, req.asset.ticker
        );
        ctx.log(Level::Info, "dex.tax_recheck", msg, None).await;
    }
    Ok(Some(orig))
}

/// Минимум 1 ед. у каждого шага (план замера и отправки после пере-замера). Ошибок нет.
pub fn unit_mins<'a>(steps: impl IntoIterator<Item = &'a mut Step>) {
    for s in steps {
        s.order.min_out = U256::from(1u8);
    }
}

/// Замер и план отправки. Принимает: окружение, запрос, план с минимумами шагов 1 ед. (собран заново после
/// unit_mins), пулы маршрута, его базовый блок, по долям (ожидаемый выход, модельный налог %), (прежняя сумма
/// минимумов, decimals USDT), исходную ошибку отката. Возвращает: план отправки (проверка баланса — база + новый
/// минимум) и строку для хронологии. Ошибки: исходная ошибка с числами замера (замер откатился, чужой пул, граница
/// ниже floor, условий ядра нет), узел, настройки.
#[allow(clippy::too_many_arguments)]
pub async fn measure(
    env: &Env,
    req: &TradeRequest,
    probe: Plan,
    entries: &[&Entry],
    at: Base,
    exp: &[(Decimal, Decimal)],
    (old, usdt_dec): (U256, u8),
    orig: GatewayError,
) -> GatewayResult<(Plan, String)> {
    let ctx = &env.ctx;
    let bal = pc::balance(probe.wallet, probe.token_out, probe.wallet);
    let res = match precheck_sim::run(ctx, &probe, at, Some(bal)).await {
        Ok(r) => r,
        Err(e) => {
            return Err(add(
                orig,
                &format!("пере-замер не выполнен — {}", e.message),
            ));
        }
    };
    let n = probe.approvals.len();
    if let Some(r) = res.iter().find(|r| !r.ok()) {
        let why = format!(
            "пере-замер: симуляция с минимумами 1 ед. откатилась — {}",
            r.why()
        );
        return Err(add(orig, &why));
    }
    let (Some(before), Some(exec), Some(after)) = (res.get(n), res.get(n + 1), res.get(n + 2))
    else {
        return Err(add(orig, "пере-замер: нет итогов симуляции"));
    };
    if let Err(why) = emitter::check(ctx, entries, &exec.logs).await {
        return Err(add(orig, &format!("пере-замер: {why}")));
    }
    let got = match (pc::u256(before), pc::u256(after)) {
        (Some(b), Some(a)) if a >= b => a - b,
        _ => {
            return Err(add(
                orig,
                "пере-замер: баланс выхода не читается или уменьшился",
            ));
        }
    };
    let Some(real) = num::to_decimal(got, usdt_dec) else {
        return Err(add(orig, "пере-замер: выход не число"));
    };
    let seen = req.remeasure.as_ref().and_then(|r| r.tax_seen_pct);
    let (bound, head) = bound_text(exp, real, seen);
    // Решение 248, п. 2: граница по замеру — ядру (закрыть монету для входа, если ниже модели больше допуска).
    exit_gap::report(ctx, &env.stats, req, exp, bound);
    let Some(rm) = req.remeasure.as_ref() else {
        let why = format!(
            "{head} — повтор с новым минимумом не разрешён ядром (первая продажа сделки или продажа без порога)"
        );
        return Err(add(orig, &why));
    };
    if bound < rm.floor {
        let why = format!(
            "{head} — граница ниже {} USDT (порог убытка, по которому решено продать), повтор не сделан",
            rm.floor.round_dp(6).normalize()
        );
        return Err(add(orig, &why));
    }
    let slip = ctx.dec("exec.dex_slippage_pct")?;
    let Some(m) = min_out_of(bound, slip, usdt_dec) else {
        return Err(add(
            orig,
            &format!("{head} — новый минимум 0, обмен без минимума не отправляется"),
        ));
    };
    let plan = probe.with_min_total(ctx, m)?;
    let d = |u: U256| num::to_decimal(u, usdt_dec).unwrap_or_default().normalize();
    let line = format!(
        "{head}, новый минимум выхода {} USDT (проверка баланса Universal Router; был {}; минимумы шагов 1 ед.; допуск {slip}%)",
        d(m),
        d(old)
    );
    let msg = format!(
        "{} {} {}: {line} (решение 248)",
        req.deal_id, req.qty, req.asset.ticker
    );
    ctx.log(Level::Warn, "dex.remeasure", msg, None).await;
    Ok((plan, line))
}

/// Дописывает к ошибке строку пере-замера. Ошибок нет.
fn add(mut e: GatewayError, what: &str) -> GatewayError {
    e.message = format!("{}; {what}", e.message);
    e
}
