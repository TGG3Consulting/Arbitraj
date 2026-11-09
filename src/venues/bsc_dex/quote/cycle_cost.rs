//! Файл: venues/bsc_dex/quote/cycle_cost.rs
//! Что делает: расчёт цикла DEX ↔ DEX на заданный вход (решение 124) — для подбора размера ядром (core/size_search.rs
//! через Venue::dex_cycle_quote) и для проверки перед сделкой. Все затраты решения 85: комиссии всех пулов, крючки и
//! проскальзывание на каждом шаге — уже в выходе (quote/cycle.rs eval, математика пулов); газ BSC свопов (замер пробы
//! каждого пула — Usable.gas) × цена газа (как при отправке: eth_gasPrice с надбавкой, не ниже минимума готовых
//! каналов); газ разрешения базовой монеты исполнителю, если его ещё нет (память разрешений wallet/allow_cache.rs, газ —
//! замер пробы первого пула, верхняя оценка — как quote/approve_gas.rs); чаевые канала (send::planned_tip) и газ их
//! транзакции перевода (send/tip_gas.rs — оценка узла, Тигран 09.11: согласовано). Налог
//! токена: через налоговые токены цикл не строится (quote/cycle_scan.rs tax_free) — структурно 0. Перевода, вывода,
//! депозита и риска движения цены за перевод у цикла НЕТ.
//! Курсы (для бюджета ядра в USDT и учёта сделки): базовая USDT — 1; WBNB — лучший пул WBNB–USDT реестра (решение 62:
//! WBNB ≈ BNB, quote/rate.rs); прочие базовые — лучший пул «базовая — USDT»; нет пула — расчёта нет (NoRoute).
//! Газ разрешений бандла (Permit2 каждого участка) здесь не оценивается — его меряет симуляция перед отправкой, и
//! проверка чистой прибыли после симуляции его учитывает (exec/cycle_trade.rs).
//! Кто вызывает: venue.rs (dex_cycle_quote), exec/{cycle_trade, cycle_dry}.rs.
//! От чего зависит: quote/{cycle, rate, route_calc}.rs, exec/cycle_gate.rs, send/mod.rs, wallet/{balances, signer}.rs,
//! check/state.rs (газ разрешения), num.rs, tokens.rs.
//! Настройки: dex.native_wrapped [bsc_dex]; через вызываемые — exec.executor_address, channel.*, send.*.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{CycleQuote, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::exec::cycle_gate::{self, Form};
use super::super::num;
use super::super::send;
use super::super::wallet::{balances, signer};
use super::graph_legs::hop_side;
use super::route_calc::Leg;
use super::{Quoting, cycle, rate};

/// Курсы для цикла: USDT за 1 базовой, USDT за 1 BNB, базовой за 1 BNB.
#[derive(Debug, Clone, Copy)]
pub struct Rates {
    pub base_usdt: Decimal,
    pub bnb_usdt: Decimal,
    pub bnb_in_base: Decimal,
}

/// Курсы по пулам реестра. Принимает: базовую монету. Ошибки: NotConfigured (dex.native_wrapped, адрес USDT), NoRoute
/// (нет пула базовая–USDT или WBNB–USDT), BadResponse (числа), узел (decimals).
pub async fn rates(q: &Quoting, base: Address) -> GatewayResult<Rates> {
    let ctx = &q.ctx;
    let usdt = q.tokens.usdt(ctx)?;
    let wbnb = ctx.addr("dex.native_wrapped")?;
    let usdt_dec = q.tokens.decimals(ctx, usdt).await?;
    let bad = || ctx.err(ErrorCode::BadResponse, "курс цикла не переводится в число");
    let one = |d: u8| U256::from(10u64).pow(U256::from(d));
    let bnb_units = rate::to_usdt(ctx, &q.reg, usdt, wbnb, one(num::BNB_DECIMALS))?;
    let bnb_usdt = num::to_decimal(bnb_units, usdt_dec).ok_or_else(bad)?;
    let base_usdt = if base == usdt {
        Decimal::ONE
    } else if base == wbnb {
        bnb_usdt
    } else {
        let d = q.tokens.decimals(ctx, base).await?;
        num::to_decimal(rate::to_usdt(ctx, &q.reg, usdt, base, one(d))?, usdt_dec)
            .ok_or_else(bad)?
    };
    if base_usdt <= Decimal::ZERO || bnb_usdt <= Decimal::ZERO {
        return Err(ctx.err(ErrorCode::NoRoute, "курс базовой или BNB в USDT — 0"));
    }
    let bnb_in_base = if base == wbnb {
        Decimal::ONE
    } else {
        bnb_usdt / base_usdt
    };
    Ok(Rates {
        base_usdt,
        bnb_usdt,
        bnb_in_base,
    })
}

/// Расчёт цикла на вход. Принимает: котировку, ноги (по памяти или свежие), базовую монету, вход, курсы, нужен ли
/// баланс кошелька. Возвращает: CycleQuote. Ошибки: NoRoute (форма не годится, шаг не исполним целиком), CoinNotChecked
/// (газ разрешения не измерен), NotConfigured (каналы, адреса), BadResponse, узел (цена газа, баланс).
pub async fn quote(
    q: &Quoting,
    legs: &[Leg],
    base: Address,
    input: Decimal,
    (r, with_wallet): (Rates, bool),
) -> GatewayResult<CycleQuote> {
    let ctx = &q.ctx;
    let form = cycle_gate::form(ctx, legs)?;
    let path = cycle::eval(ctx, legs, input)?;
    let wei = q
        .gas
        .wei(ctx, &q.heads)
        .await?
        .max(send::planned_floor(ctx)?);
    let swap_units: u64 = path.hops.iter().map(|h| h.usable.gas).sum();
    let approve_units = approve_units(q, legs, form, base, input).await?;
    let gas = wei.saturating_mul(U256::from(swap_units.saturating_add(approve_units)));
    let bad = || ctx.err(ErrorCode::BadResponse, "газ цикла не помещается в Decimal");
    let tip = send::planned_tip(ctx)
        .and_then(|w| num::to_decimal(w, num::BNB_DECIMALS))
        .unwrap_or(Decimal::ZERO);
    // Тигран 09.11 (согласовано): газ отдельной транзакции перевода чаевых (send/tip_gas.rs, та же цена газа).
    let tip_gas = send::tip_gas::planned_bnb(ctx, wei)
        .await?
        .unwrap_or(Decimal::ZERO);
    let cost_bnb = num::to_decimal(gas, num::BNB_DECIMALS).ok_or_else(bad)? + tip + tip_gas;
    let cost_base = cost_bnb * r.bnb_in_base;
    let wallet_base = if with_wallet {
        let w = signer::address(ctx)?;
        let units = balances::token_units(ctx, base, w).await?;
        let d = q.tokens.decimals(ctx, base).await?;
        Some(num::to_decimal(units, d).ok_or_else(bad)?)
    } else {
        None
    };
    Ok(CycleQuote {
        input: path.coins,
        output: path.usdt,
        cost_bnb,
        cost_base,
        net_base: path.usdt - path.coins - cost_base,
        base_usdt: r.base_usdt,
        bnb_usdt: r.bnb_usdt,
        wallet_base,
        form: form.text(),
    })
}

/// Газ разрешения базовой монеты исполнителю, если его нет (единицы газа; 0 — разрешение есть или форма — бандл).
/// Ошибки: CoinNotChecked — газ разрешения не измерен пробой первого пула; ключ кошелька, узел.
async fn approve_units(
    q: &Quoting,
    legs: &[Leg],
    form: Form,
    base: Address,
    input: Decimal,
) -> GatewayResult<u64> {
    let ctx = &q.ctx;
    let Form::Executor(x) = form else {
        return Ok(0);
    };
    let first = &legs[0];
    let need = num::to_units(input, first.dec_in)
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "вход цикла не в единицах"))?;
    let wallet = signer::address(ctx)?;
    if q.allow.erc20(ctx, base, wallet, x).await? >= need {
        return Ok(0);
    }
    let f = q.checks.pool(&first.entry.pool.reference);
    let g = match hop_side(&first.entry, base) {
        Some(Side::Buy) => f.gas_approve_buy,
        _ => f.gas_approve_sell,
    };
    g.ok_or_else(|| {
        let why = "газ разрешения базовой монеты исполнителю не измерен пробой первого пула цикла";
        ctx.err(ErrorCode::CoinNotChecked, why)
    })
}
