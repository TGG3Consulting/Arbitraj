//! Файл: venues/bsc_dex/exec/executor/call.rs
//! Что делает: вызов execute исполнителя для маршрута из долей (решение 122, часть X). Доли — шаги exec/chain.rs (как
//! у бандла, exec/bundle.rs): у каждой доли первый шаг — вход доли (amount_in, единицы токена входа), последний — её
//! минимум (min_out, из верификации, решение 83). Шаги исполнителя идут доля за долей: первый шаг доли берёт свою часть
//! ТЕКУЩЕГО остатка входа (bps = вход доли / ещё не распределённый вход; последняя доля — весь остаток, 10000), каждый
//! следующий шаг доли — весь промежуточный токен (10000: предыдущая доля его уже израсходовала). Вход сделки — сумма
//! входов долей; минимум сделки — сумма минимумов долей (контракт проверяет то, что реально получил кошелёк).
//! Кто вызывает: exec/executor/{trade, dry}.rs.
//! От чего зависит: exec/executor/{abi, kind, entry}.rs, exec/chain.rs (Step), ctx.rs.
//! Настройки: dex.deadline_s [bsc_dex] (срок транзакции, как у Universal Router).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::ctx::Ctx;
use super::super::chain::Step;
use super::abi::ArbExecutor;
use super::entry;
use super::kind::{self, ALL_BPS, StepSpec};

/// Вызов execute: всё, что уходит в транзакцию.
#[derive(Debug, Clone)]
pub struct ExecCall {
    /// Адрес исполнителя (развёрнутый — exec.executor_address; в сухой сделке — подставленный).
    pub executor: Address,
    pub token_in: Address,
    pub amount_in: U256,
    pub token_out: Address,
    pub min_out: U256,
    pub deadline: U256,
    pub steps: Vec<ArbExecutor::Step>,
}

impl ExecCall {
    /// Данные транзакции: execute(tokenIn, amountIn, tokenOut, minOut, deadline, steps). Ошибок нет.
    pub fn data(&self) -> Bytes {
        Bytes::from(
            ArbExecutor::executeCall {
                tokenIn: self.token_in,
                amountIn: self.amount_in,
                tokenOut: self.token_out,
                minOut: self.min_out,
                deadline: self.deadline,
                steps: self.steps.clone(),
            }
            .abi_encode(),
        )
    }
}

/// Собирает вызов. Принимает: доли (шаги exec/chain.rs с минимумом доли в последнем шаге), адрес исполнителя, время
/// блока (секунды). Ошибки: NoRoute (нет долей, у долей разный вход/выход, пул без шага исполнителя), QuoteMismatch
/// (минимум 0, доля меньше 1 bps), NotConfigured (dex.deadline_s, комиссия V2).
pub fn build(
    ctx: &Ctx,
    legs: &[Vec<Step>],
    executor: Address,
    now: u64,
) -> GatewayResult<ExecCall> {
    let ends: Vec<(&Step, &Step)> = legs
        .iter()
        .filter_map(|l| Some((l.first()?, l.last()?)))
        .collect();
    let Some(&(first, last)) = ends.first() else {
        return Err(ctx.err(ErrorCode::NoRoute, "исполнитель: маршрут без долей"));
    };
    if ends.len() != legs.len() {
        return Err(ctx.err(ErrorCode::NoRoute, "исполнитель: доля без шагов"));
    }
    let (token_in, token_out) = (first.order.token_in, last.order.token_out);
    // Ревью стейблсвапов: цикл (вход = выход) — одна доля, как требует контракт («loop»): иначе выход вида с выходом на
    // исполнитель (CURVE / STABLE_U256 / SADDLE) посчитался бы входом следующей доли.
    if token_in == token_out && legs.len() > 1 {
        let why = format!(
            "исполнитель: цикл (вход = выход) из {} долей — контракт исполняет цикл только одной долей",
            legs.len()
        );
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    let same =
        |(a, b): &(&Step, &Step)| a.order.token_in == token_in && b.order.token_out == token_out;
    if !ends.iter().all(same) {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "исполнитель: у долей разный вход или выход",
        ));
    }
    let amount_in = ends
        .iter()
        .fold(U256::ZERO, |a, (f, _)| a.saturating_add(f.order.amount_in));
    let min_out = ends
        .iter()
        .fold(U256::ZERO, |a, (_, l)| a.saturating_add(l.order.min_out));
    if min_out.is_zero() {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "исполнитель: минимум выхода 0 — сделки нет",
        ));
    }
    let mut steps = Vec::new();
    let mut left = amount_in;
    for (n, leg) in legs.iter().enumerate() {
        let first_in = leg[0].order.amount_in;
        let share = if n + 1 == legs.len() {
            ALL_BPS
        } else {
            bps(ctx, first_in, left)?
        };
        left = left.saturating_sub(first_in);
        for (i, s) in leg.iter().enumerate() {
            let (params, pool) = entry::params(ctx, &s.entry, s.order.token_in)?;
            let spec = StepSpec {
                params,
                pool,
                token_in: s.order.token_in,
                token_out: s.order.token_out,
                share_bps: if i == 0 { share } else { ALL_BPS },
            };
            steps.push(kind::encode(&spec).map_err(|w| ctx.err(ErrorCode::NoRoute, w))?);
        }
    }
    Ok(ExecCall {
        executor,
        token_in,
        amount_in,
        token_out,
        min_out,
        deadline: U256::from(now + ctx.int("dex.deadline_s")?),
        steps,
    })
}

/// Доля входа в bps от ещё не распределённого: part × 10000 / left, вниз (остаток уходит последней доле).
/// Ошибки: QuoteMismatch — доля 0 bps (меньше 0,01 % остатка) или больше остатка.
fn bps(ctx: &Ctx, part: U256, left: U256) -> GatewayResult<u16> {
    let b = part
        .checked_mul(U256::from(ALL_BPS))
        .and_then(|x| x.checked_div(left))
        .unwrap_or_default();
    match u16::try_from(b) {
        Ok(v) if v > 0 && v <= ALL_BPS => Ok(v),
        _ => Err(ctx.err(
            ErrorCode::QuoteMismatch,
            format!("исполнитель: доля {part} из остатка {left} не выражается в 1…10000 bps"),
        )),
    }
}
