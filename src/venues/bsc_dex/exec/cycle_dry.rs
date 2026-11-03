//! Файл: venues/bsc_dex/exec/cycle_dry.rs
//! Что делает: «Сухая сделка DEX» в режиме «цикл» (решение 124; сухая сделка — решение 121 C4): цикл из сигнала,
//! вход и граница minOut ядра (core/cycle_plan.rs) → та же сборка, что у сделки (exec/cycle_trade.rs: свежее перечтение
//! пулов, форма исполнитель / бандл) → одна симуляция eth_simulateV1 от кошелька (exec/dry_sim.rs: не хватает базовой
//! — перевод из пула вне цикла внутри симуляции; разрешения — первыми вызовами) → выход и газ по симуляции → чистая
//! прибыль против dex_dex.min_profit_usd. Режим «через исполнитель» (решение 122): код ArbExecutor и владелец
//! подставляются на будущий адрес развёртывания — проверка цикла через исполнитель до развёртывания.
//! Ничего не подписывает и не отправляет, nonce и очередь кошелька не трогает, уведомлений сделки не шлёт.
//! У цикла вход и выход — один токен: dry_sim меряет прирост базовой у кошелька, «выход по симуляции» = вход + прирост
//! (при убытке исполнитель и BALANCE_CHECK откатывают обмен — отказ с текстом узла).
//! Кто вызывает: venue.rs (dex_dry_cycle).
//! От чего зависит: exec/{cycle_gate, cycle_steps, cycle_bundle, dry_build, dry_sim}.rs, exec/executor/{dry, mod}.rs,
//! quote/{cycle_legs, cycle_cost}.rs, wallet/signer.rs.
//! Настройки: через вызываемые — exec.executor_address, verify.*, dex.*, dex_dex.min_profit_usd, dex.multicall3.

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{Asset, CycleTradeRequest, DexDrySwap, DrySwapPool, Side};
use crate::gateway::errors::GatewayResult;

use super::super::check::simulate::Base;
use super::super::num;
use super::super::quote::{Quoting, cycle_cost, cycle_legs};
use super::super::wallet::signer;
use super::cycle_gate::{self, Form};
use super::cycle_steps::{self, exec_steps, net_check, units};
use super::dry_build::{Built, Ready};
use super::dry_sim::{self, Io};
use super::executor::{self, dry::DryMode};
use super::{cycle_bundle, cycle_exec};

/// Сухая сделка цикла. Принимает: котировку, запрос (вход и граница ядра), режим. Возвращает: итог (отказ — reason).
/// Ошибок нет.
pub async fn run(q: &Quoting, req: CycleTradeRequest, mode: DryMode) -> DexDrySwap {
    let asset = Asset {
        ticker: req.cycle.base_ticker.clone(),
        bsc_contract: Some(req.cycle.base),
    };
    let mut r = DexDrySwap::new(q.ctx.venue(), asset, Side::Sell, Decimal::ZERO);
    r.qty = Some(req.input);
    r.out_symbol = Some(req.cycle.base_ticker.clone());
    r.min_out = Some(req.min_out);
    r.pools = req
        .cycle
        .hops
        .iter()
        .map(|h| DrySwapPool {
            dex: h.family.clone(),
            kind: h.kind,
            pool: h.pool.clone(),
            leg: 0,
        })
        .collect();
    r.notes.push(
        "цикл: вход и выход — базовая монета; «выход по симуляции» = вход + прирост базовой у кошелька".into(),
    );
    if let Err(e) = fill(q, &req, mode, &mut r).await {
        r.reason = Some(e.message);
    }
    r
}

/// Тело run. Ошибки: любая на пути — становится причиной.
async fn fill(
    q: &Quoting,
    req: &CycleTradeRequest,
    mode: DryMode,
    r: &mut DexDrySwap,
) -> GatewayResult<()> {
    let ctx = &q.ctx;
    let wallet = signer::address(ctx)?;
    let base = req.cycle.base;
    r.quote_out = cycle_legs::now(q, &req.cycle)
        .await
        .and_then(|l| super::super::quote::cycle::eval(ctx, &l, req.input))
        .ok()
        .map(|p| p.usdt);
    let (legs, _) = cycle_legs::fresh(q, &req.cycle).await?;
    let rates = cycle_cost::rates(q, base).await?;
    r.usdt = req.input * rates.base_usdt;
    let dec = legs[0].dec_in;
    let (steps, expected) = exec_steps(ctx, &legs, req.input, req.min_out)?;
    r.live_out = Some(expected);
    cycle_exec::below(ctx, expected, req.min_out)?;
    let (at, now) = q.heads.at(ctx).await?;
    let form = match mode {
        DryMode::Executor => None,
        DryMode::AsTrade => Some(cycle_gate::form(ctx, &legs)?),
    };
    let (built, base_at, form_text) = match form {
        Some(Form::Bundle) => {
            let floor = units(ctx, req.min_out, dec)?;
            let a =
                cycle_bundle::build_floor(ctx, &legs, (req.input, floor), (wallet, 0, now, false))
                    .await?;
            (Built::Many(a.bundle), Base::from(at), Form::Bundle.text())
        }
        _ => {
            let deployed = executor::address(ctx)?;
            let x = executor::dry::build(
                ctx,
                std::slice::from_ref(&steps),
                wallet,
                now,
                (mode, deployed),
            )
            .await?;
            let t = format!("цикл: {}", x.form);
            (Built::Exec(x), Base::timed(at, now), t)
        }
    };
    r.form = Some(form_text.clone());
    r.block = Some(at);
    let ready = Ready {
        built,
        legs: vec![steps],
        coins: req.input,
        usdt: expected,
        form: form_text,
        at: base_at,
    };
    let need = units(ctx, req.input, dec)?;
    let io = Io {
        token_in: Some(base),
        need,
        taxes_in: None,
        token_out: Some(base),
    };
    let sim = dry_sim::run(q, &ready, wallet, &io).await?;
    r.gas_swap = Some(sim.gas_swap);
    r.gas_approve = Some(sim.gas_approve);
    r.approvals = sim.approvals;
    r.funding = sim.funding;
    if let Some(why) = sim.reason {
        r.reason = Some(why);
        return Ok(());
    }
    let gain = sim.out.unwrap_or(U256::ZERO);
    let out = num::to_decimal(need.saturating_add(gain), dec).unwrap_or_default();
    r.sim_out = Some(out);
    let gas = sim.gas_swap.saturating_add(sim.gas_approve);
    let net = net_check(
        ctx,
        (out, req.input),
        (gas, cycle_steps::wei(q).await?),
        &rates,
    )?;
    r.notes.push(format!(
        "чистая прибыль по симуляции {net} {} (≈ {} USDT), газ {gas} ед. вместе с разрешениями",
        req.cycle.base_ticker,
        (net * rates.base_usdt).round_dp(6)
    ));
    r.ok = out >= req.min_out;
    if !r.ok {
        r.reason = Some(format!(
            "выход по симуляции {out} < границы {}",
            req.min_out
        ));
    }
    Ok(())
}
