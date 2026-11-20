//! Файл: venues/bsc_dex/exec/tip_cycle.rs
//! Что делает: решение 437 (3) — динамические чаевые сделки DEX ↔ DEX через исполнитель в сети eip1559_l1 (режим
//! send.tip_mode = profit_share):
//! - choose — до проверки порога и отправки: P0 = выход − вход − газ без чаевых (exec/cycle_gas.rs Bill с чаевыми 0:
//!   газ_L2 × база + газ_L1 × база), в нативной монете по курсу пулов (quote/cycle_cost.rs Rates); Pmin —
//!   dex_dex.min_profit_usd в нативной монете; доля s — exec/tip_state.rs; чаевые — exec/tip_pick.rs. Всё нужное после
//!   отправки (пределы, срок ожидания блока, пулы маршрута) читается здесь, до неё (решение 398);
//! - after — после итога отправки, в фоновой задаче (сделка не ждёт разбора и записи): в блоке без отката — победа;
//!   откат — разбор по блоку включения (exec/tip_judge.rs, место из квитанции); отказ условия −32003 (решение 436,
//!   send/sequencer_cond.rs REJECTED) — разбор по ближайшему блоку после отправки (ждём его не дольше
//!   feed.stale_after_ms); прочие ошибки и «исход неизвестен» — долю не трогают.
//!
//! Режим fixed, сеть legacy (BSC), бандл — без изменений (choose возвращает Bill как есть, after ничего не делает).
//! Кто вызывает: exec/cycle_exec.rs.
//! От чего зависит: exec/{cycle_gas, tip_pick, tip_state, tip_judge, txs}.rs, send/{tip_mode, sequencer_cond}.rs,
//! quote/cycle_cost.rs (Rates), num.rs, ctx.rs.
//! Настройки: send.tip_mode и настройки profit_share (send/tip_mode.rs), dex_dex.min_profit_usd, feed.stale_after_ms
//! [площадка DEX].

use std::time::Duration;

use alloy_primitives::B256;
use rust_decimal::Decimal;

use crate::core::types::{PoolRef, TipOutcome, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::num;
use super::super::quote::Quoting;
use super::super::quote::cycle_cost::Rates;
use super::super::quote::route_calc::Leg;
use super::super::send::sequencer_cond;
use super::super::send::tip_mode::{self, ShareCfg, TipMode};
use super::cycle_gas::Bill;
use super::tip_judge::{self, Probe};
use super::tip_state;
use super::txs::{Done, Env};

/// Выбранные чаевые и всё для разбора итога (прочитано до отправки).
#[derive(Debug, Clone)]
pub struct TipPlan {
    pub cfg: ShareCfg,
    /// Чаевые, wei за газ.
    pub tip: u128,
    /// feed.stale_after_ms — сколько ждать блока после отправки (отказ условия).
    pub stale: Duration,
    pub pools: Vec<PoolRef>,
}

/// Чаевые сделки цикла. Принимает: котировку, газ транзакции (цена по режиму), (выход, вход) в базовой, курсы, ноги.
/// Возвращает: Bill с выбранными чаевыми и TipPlan; не profit_share или не eip1559_l1 — Bill как есть и None.
/// Ошибки: NotConfigured (режим, пределы, dex_dex.min_profit_usd, feed.stale_after_ms), QuoteMismatch — предел по
/// прибыли ниже нижнего предела чаевых (сделки нет, причина), BadResponse (числа).
pub async fn choose(
    q: &Quoting,
    bill: Bill,
    (out, input): (Decimal, Decimal),
    r: &Rates,
    legs: &[Leg],
) -> GatewayResult<(Bill, Option<TipPlan>)> {
    let ctx = &q.ctx;
    let Bill::Split { l2, .. } = bill else {
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
    let plan = TipPlan {
        cfg,
        tip,
        stale: Duration::from_millis(ctx.int("feed.stale_after_ms")?),
        pools: legs.iter().map(|l| l.entry.pool.reference.clone()).collect(),
    };
    Ok((bill.with_tip(tip), Some(plan)))
}

/// Итог отправки для доли чаевых — в фоновой задаче (сделка не ждёт). Принимает: окружение, план (None — ничего не
/// делается), итог send_tipped, голова на момент отправки. Ошибок нет: сбой разбора — строка журнала send.tip_loss,
/// доля не меняется.
pub fn after(env: &Env, plan: Option<TipPlan>, res: &GatewayResult<Done>, sent_head: u64) {
    let Some(plan) = plan else { return };
    let probe = match res {
        Ok(d) if d.chain.outcome == TxOutcome::Included => None,
        Ok(d) if d.chain.outcome == TxOutcome::Reverted => {
            let at = d.receipt.as_ref().and_then(|x| x.transaction_index);
            Some(Probe {
                ours: d.chain.hash,
                block: d.chain.block.unwrap_or(sent_head),
                index: at.map(|i| i.to()),
                tip: plan.tip,
                pools: plan.pools.clone(),
            })
        }
        Err(e) if e.message.contains(sequencer_cond::REJECTED) => Some(Probe {
            ours: B256::ZERO,
            block: sent_head + 1,
            index: None,
            tip: plan.tip,
            pools: plan.pools.clone(),
        }),
        _ => return,
    };
    let (ctx, stats, heads) = (env.ctx.clone(), env.stats.clone(), env.heads.clone());
    tokio::spawn(async move {
        let Some(p) = probe else {
            let won = (plan.tip, TipOutcome::Win);
            return tip_state::record(&ctx, &stats, &plan.cfg, won, None).await;
        };
        // Блок поиска должен быть у своего узла (отказ условия — следующий блок может ещё не прийти).
        let mut rx = heads.watch();
        let reach = async {
            loop {
                let got = rx.borrow().as_ref().is_some_and(|h| h.number >= p.block);
                if got || rx.changed().await.is_err() {
                    break;
                }
            }
        };
        let _ = tokio::time::timeout(plan.stale, reach).await;
        match tip_judge::judge(&ctx, &p).await {
            Ok(l) => {
                let out = (plan.tip, l.outcome);
                tip_state::record(&ctx, &stats, &plan.cfg, out, Some(l)).await;
            }
            Err(e) => {
                let why = &e.message;
                let msg = format!("разбор проигрыша (блок {}) не удался: {why}", p.block);
                ctx.log(Level::Warn, "send.tip_loss", msg, None).await;
            }
        }
    });
}
