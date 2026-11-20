//! Файл: venues/bsc_dex/exec/tip_cycle.rs
//! Что делает: решение 437 (3) — динамические чаевые сделки DEX ↔ DEX через исполнитель в сети eip1559_l1 (режим
//! send.tip_mode = profit_share):
//! - choose — до проверки порога и отправки: P0 = выход − вход − газ без чаевых (exec/cycle_gas.rs Bill с чаевыми 0:
//!   газ_L2 × база + газ_L1 × база), в нативной монете по курсу пулов (quote/cycle_cost.rs Rates); Pmin —
//!   dex_dex.min_profit_usd в нативной монете; доля s — exec/tip_state.rs; чаевые — exec/tip_pick.rs. Всё нужное после
//!   отправки (пределы, срок ожидания блока, пулы маршрута) читается здесь, до неё (решение 398);
//! - after — после итога отправки, в фоновой задаче (сделка не ждёт разбора и записи): в блоке без отката — победа;
//!   откат — разбор по блоку включения (exec/tip_judge.rs, место из квитанции); отказ −32003 по условию пулов
//!   (решение 438, send/sequencer_cond.rs REJECTED_POOLS) — разбор по окну (блок плана, голова своего узла на момент
//!   ответа]: первый изменивший пул маршрута (ревью 437, п. 2); окно не определено (нет блока плана, голова не новее)
//!   или в нём никого — доля не меняется, только журнал send.tip_loss; отказ −32003 иного условия (окно времени) —
//!   не проигрыш конкуренту: журнал, доля не меняется; прочие ошибки и «исход неизвестен» — долю не трогают.
//!
//! Ревью 437, п. 4: чаевые для pick, net_check и отправки — не ниже (минимум каналов − база), как подпишет
//! send/tx_price.rs floored (exec/tx_fee.rs).
//!
//! Режим fixed, сеть legacy (BSC), бандл — без изменений (choose возвращает Bill как есть, after ничего не делает).
//! Кто вызывает: exec/cycle_exec.rs.
//! От чего зависит: exec/{cycle_gas, tip_pick, tip_state, tip_judge, txs, pool_cond_topics}.rs, send/{tip_mode, sequencer_cond, mod}.rs,
//! quote/cycle_cost.rs (Rates), num.rs, ctx.rs.
//! Настройки: send.tip_mode и настройки profit_share (send/tip_mode.rs), dex_dex.min_profit_usd [площадка DEX];
//! минимум каналов — send::planned_floor (channel.*).

use alloy_primitives::B256;
use rust_decimal::Decimal;

use crate::core::types::{PoolRef, TipOutcome, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::num;
use super::super::quote::Quoting;
use super::super::quote::cycle_cost::Rates;
use super::super::quote::route_calc::Leg;
use super::super::send::tip_mode::{self, ShareCfg, TipMode};
use super::super::send::{self, sequencer_cond};
use super::cycle_gas::Bill;
use super::pool_cond_topics;
use super::tip_judge::{self, Probe};
use super::tip_state;
use super::txs::{Done, Env};

/// Выбранные чаевые и всё для разбора итога (прочитано до отправки).
#[derive(Debug, Clone)]
pub struct TipPlan {
    pub cfg: ShareCfg,
    /// Чаевые, wei за газ.
    pub tip: u128,
    pub pools: Vec<PoolRef>,
    /// Ревью 438 (D): topic0 событий, пишущих слоты условия, по пулам (как pools; None — любое событие).
    pub topics: Vec<Option<Vec<B256>>>,
}

/// Чаевые сделки цикла. Принимает: котировку, газ транзакции (цена по режиму), (выход, вход) в базовой, курсы, ноги.
/// Возвращает: Bill с выбранными чаевыми и TipPlan; не profit_share или не eip1559_l1 — Bill как есть и None.
/// Ошибки: NotConfigured (режим, пределы, dex_dex.min_profit_usd, каналы), QuoteMismatch — предел по
/// прибыли ниже нижнего предела чаевых (сделки нет, причина), BadResponse (числа).
pub async fn choose(
    q: &Quoting,
    bill: Bill,
    (out, input): (Decimal, Decimal),
    r: &Rates,
    legs: &[Leg],
) -> GatewayResult<(Bill, Option<TipPlan>)> {
    let ctx = &q.ctx;
    let Bill::Split { l2, base, .. } = bill else {
        return Ok((bill, None));
    };
    let TipMode::ProfitShare(cfg) = tip_mode::read(ctx)? else {
        return Ok((bill, None));
    };
    let bad = || ctx.err(ErrorCode::BadResponse, "газ цикла без чаевых не в Decimal");
    let gas0 = num::to_decimal(bill.with_tip(0).wei(), num::BNB_DECIMALS).ok_or_else(bad)?;
    let p0 = (out - input) / r.bnb_in_base - gas0;
    let pmin = ctx.dec("dex_dex.min_profit_usd")? / r.bnb_usdt;
    let s = tip_state::share(ctx, &cfg).await;
    let tip = match super::tip_pick::pick(&cfg, s, (p0, pmin), l2) {
        Ok(t) => t,
        Err(why) => {
            let why = format!("чаевые (доля {s} %): {why}");
            return Err(ctx.err(ErrorCode::QuoteMismatch, why));
        }
    };
    // Ревью 437, п. 4: минимум каналов поднимает чаевые до (минимум − база) — как подпишет tx_price::floored.
    let lift = send::planned_floor(ctx)?.saturating_sub(base);
    let tip = tip.max(u128::try_from(lift).unwrap_or(u128::MAX));
    let plan = TipPlan {
        cfg,
        tip,
        pools: legs.iter().map(|l| l.entry.pool.reference.clone()).collect(),
        topics: legs.iter().map(|l| pool_cond_topics::topics(&l.entry)).collect(),
    };
    Ok((bill.with_tip(tip), Some(plan)))
}

/// Итог отправки для доли чаевых — в фоновой задаче (сделка не ждёт). Принимает: окружение, план (None — ничего не
/// делается), итог send_tipped, блок плана условия пулов (решение 438; None — условия пулов не было). Ошибок нет: сбой
/// или неоднозначный разбор — строка журнала send.tip_loss, доля не меняется.
pub fn after(
    env: &Env,
    plan: Option<TipPlan>,
    res: &GatewayResult<Done>,
    plan_block: Option<u64>,
) {
    let Some(plan) = plan else { return };
    let ctx = &env.ctx;
    let mk = |blocks, ours, index| Probe {
        ours,
        blocks,
        index,
        tip: plan.tip,
        pools: plan.pools.clone(),
        topics: plan.topics.clone(),
    };
    let probe = match res {
        Ok(d) if d.chain.outcome == TxOutcome::Included => None,
        Ok(d) if d.chain.outcome == TxOutcome::Reverted => {
            let at = d.receipt.as_ref().and_then(|x| x.transaction_index);
            let Some(b) = d.chain.block else {
                return note(ctx, "откат без номера блока включения — разбор невозможен".into());
            };
            Some(mk((b, b), d.chain.hash, at.map(|i| i.to())))
        }
        Err(e) if e.message.contains(sequencer_cond::REJECTED_POOLS) => {
            let head = env.heads.latest().map(|h| h.number);
            match (plan_block, head) {
                (Some(p), Some(h)) if h > p => Some(mk((p + 1, h), B256::ZERO, None)),
                _ => {
                    let why = format!(
                        "отказ условия пулов: окно блоков не определено (блок плана {plan_block:?}, голова \
                         {head:?}) — доля не меняется"
                    );
                    return note(ctx, why);
                }
            }
        }
        Err(e) if e.message.contains(sequencer_cond::REJECTED) => {
            let why = "отказ условия −32003 не по пулам (окно времени timestampMax) — не проигрыш конкуренту, \
                       доля не меняется";
            return note(ctx, why.into());
        }
        _ => return,
    };
    let (ctx, stats) = (env.ctx.clone(), env.stats.clone());
    tokio::spawn(async move {
        let Some(p) = probe else {
            let won = (plan.tip, TipOutcome::Win);
            return tip_state::record(&ctx, &stats, &plan.cfg, won, None).await;
        };
        let (from, to) = p.blocks;
        match tip_judge::judge(&ctx, &p).await {
            Ok(Some(l)) => {
                let out = (plan.tip, l.outcome);
                tip_state::record(&ctx, &stats, &plan.cfg, out, Some(l)).await;
            }
            Ok(None) => {
                let msg = format!(
                    "отказ условия пулов: в блоках {from}–{to} изменившего пул маршрута нет — доля не меняется"
                );
                ctx.log(Level::Warn, "send.tip_loss", msg, None).await;
            }
            Err(e) => {
                let why = &e.message;
                let msg = format!("разбор проигрыша (блоки {from}–{to}) не удался: {why}");
                ctx.log(Level::Warn, "send.tip_loss", msg, None).await;
            }
        }
    });
}

/// Строка журнала send.tip_loss без изменения доли — в фоне. Принимает: окружение, текст. Ошибок нет.
fn note(ctx: &super::super::ctx::Ctx, msg: String) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        ctx.log(Level::Warn, "send.tip_loss", msg, None).await;
    });
}
