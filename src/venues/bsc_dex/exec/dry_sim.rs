//! Файл: venues/bsc_dex/exec/dry_sim.rs
//! Что делает: симуляция сухой сделки DEX (решение 121, C4) — одна последовательность eth_simulateV1 поверх блока
//! сборки от имени кошелька бота: [перевод входа от пула-источника (решение 302: или держателя-синглтона при
//! presell.fund_fallback; решение 380 — от нескольких источников вместе), если у кошелька не хватает — exec/dry_fund.rs] →
//! balanceOf входа (сколько у кошелька) → баланс выхода до → разрешения (approve / Permit2, если нужны) → обмен
//! (execute Universal Router; у бандла — все его транзакции по порядку, разметка exec/bundle_sim.rs) → баланс выхода
//! после. При пополнении (решение 252, вариант «б», Тигран 10.11) — ещё замер излишка пар первого шага (balanceOf −
//! резерв до и после перевода, резервы после обмена; только чтение — exec/dry_surplus.rs) и поправка выхода на прирост,
//! созданный самим пополнением (exec/dry_surplus_cut.rs). Выход по симуляции = после − до [− поправка]; газ — gasUsed
//! вызовов; «кто меняет» (решение 59) — check/emitter::missing
//! БЕЗ исключения пула (сухая сделка ничего не меняет в справочнике). Ничего не подписывается и не отправляется.
//! Выход BNB (покупка нативной монеты, UNWRAP_WETH) — Multicall3.getEthBalance (dex.multicall3); не задан — не измерен.
//! Через исполнитель ArbExecutor (решение 122, часть X): разрешение ERC-20 исполнителю → execute; не развёрнут —
//! его код и слот владельца подставляются подменой состояния (check/simulate.rs::run_with, stateOverrides).
//! Решение 407 (t192): Io.credit (только симуляция продажи до покупки — exec/presell.rs) — входа у кошелька меньше
//! нужного: вместо пополнения слот баланса кошелька = need подменой состояния (exec/presell_slot.rs); balanceOf входа
//! после подмены ≠ need ровно — Sim.credit_bad (подмена недостоверна, presell повторяет без неё).
//! Разбор итогов — exec/dry_judge.rs.
//! Кто вызывает: exec/dry_swap.rs.
//! От чего зависит: check/simulate.rs, check/probe_calls.rs, check/emitter.rs, exec/{dry_build, dry_fund, dry_surplus,
//! dry_surplus_cut, bundle_sim, plan, ur}.rs, wallet/balances.rs, node/multicall.rs (адрес).
//! Настройки: через вызываемые — dex.multicall3.
//!
//! eth_simulateV1 — execution-apis 16caee27 src/eth/execute.yaml#L137-L260; getEthBalance —
//! https://github.com/mds1/multicall3/blob/main/src/Multicall3.sol (function getEthBalance(address)).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::TokenTaxes;
use crate::gateway::errors::GatewayResult;

use super::super::check::probe_calls as pc;
use super::super::check::simulate::{self, Call, Override};
use super::super::node::multicall;
use super::super::quote::Quoting;
use super::super::wallet::balances;
use super::bundle_sim::build_sim;
use super::dry_build::{Built, Ready};
use super::dry_fund;
use super::dry_judge::judge;
use super::dry_surplus;
use super::dry_surplus_cut::{self, Surplus};

alloy::sol! {
    interface IMulticall3Eth {
        function getEthBalance(address addr) external view returns (uint256 balance);
    }
}

/// Что вошло в сделку: токен входа (None — платит роутер из BNB транзакции), сколько, налоги входа, токен выхода
/// (None — нативный BNB).
pub struct Io<'a> {
    pub token_in: Option<Address>,
    pub need: U256,
    pub taxes_in: Option<&'a TokenTaxes>,
    pub token_out: Option<Address>,
    /// Решение 407: подмена баланса входа кошелька (stateDiff токена: слот = need); None — пополнение как раньше.
    pub credit: Option<Override>,
    /// Ревью t192 Н2: баланс входа кошелька, уже прочитанный на блоке симуляции (exec/presell_credit.rs);
    /// None — читать.
    pub have: Option<U256>,
}

/// Итог симуляции.
#[derive(Default)]
pub struct Sim {
    pub out: Option<U256>,
    pub gas_swap: u64,
    pub gas_approve: u64,
    pub approvals: usize,
    pub funding: Option<String>,
    /// Излишек пары от пополнения (решение 252, вариант «б»): out уже уменьшен на cut; None — прироста нет.
    pub surplus: Option<Surplus>,
    /// Причина отказа; None — все вызовы прошли и обмен сделали пулы маршрута.
    pub reason: Option<String>,
    /// Решение 380: пополнить на полную сумму нечем — сколько единиц входа МОЖНО дать кошельку (вместе с его балансом);
    /// None — пополнение не было причиной отказа.
    pub fundable: Option<U256>,
    /// Решение 407: подмена баланса применена, но balanceOf входа ≠ need ровно — подмена недостоверна.
    pub credit_bad: bool,
}

/// Симуляция. Принимает: котировку, собранную сделку, кошелёк, вход и выход. Возвращает: Sim (отказ — reason).
/// Ошибки: узел (метод не поддерживается, сбой), BadResponse.
pub async fn run(q: &Quoting, r: &Ready, wallet: Address, io: &Io<'_>) -> GatewayResult<Sim> {
    run_anchored(q, r, wallet, io, None).await
}

pub(super) async fn run_anchored(
    q: &Quoting,
    r: &Ready,
    wallet: Address,
    io: &Io<'_>,
    parent: Option<alloy_primitives::B256>,
) -> GatewayResult<Sim> {
    let ctx = &q.ctx;
    let mut sim = Sim::default();
    let mut calls: Vec<Call> = Vec::new();
    let mut values: Vec<(usize, U256)> = Vec::new();
    let (mut watch, mut funded, mut credited) = (Vec::new(), U256::ZERO, false);
    if let Some(t) = io.token_in {
        let have = match io.have {
            Some(h) => h,
            None => balances::token_units(ctx, t, wallet).await?,
        };
        if have >= io.need {
            sim.funding = Some(format!("баланс кошелька ({have} ≥ {} единиц)", io.need));
        } else if io.credit.is_some() {
            // Решение 407: баланс входа — подменой слота (exec/presell_slot.rs), источники не нужны.
            sim.funding = Some(format!(
                "подмена баланса кошелька в симуляции: {have} → {} единиц (stateDiff слота баланса, слот проверен \
                 balanceOf; решение 407)",
                io.need
            ));
            credited = true;
        } else {
            let fs = match dry_fund::plan(q, r, t, (have, io.need), io.taxes_in).await? {
                Ok(v) => v,
                Err(short) => {
                    sim.reason = Some(short.why);
                    sim.fundable = Some(short.fundable);
                    return Ok(sim);
                }
            };
            // Решение 252, вариант «б»: излишек пар первого шага до и после пополнения (exec/dry_surplus.rs); решение
            // 380 — вокруг всех переводов пополнения (источников может быть несколько).
            watch = dry_surplus::around(
                &mut calls,
                (r, wallet, t),
                fs.iter()
                    .map(|f| pc::transfer(f.src, t, f.to.unwrap_or(wallet), f.amount))
                    .collect(),
            );
            // На кошелёк уходит только перевод без «куда» (один источник или узел набора — решение 380, Н4).
            funded = fs
                .iter()
                .filter(|f| f.to.is_none())
                .fold(U256::ZERO, |s, f| s.saturating_add(f.amount));
            // Источник виден в журнале (решение 302): «из пула …» или «из держателя-синглтона (…) …»; решение 380 —
            // «из K источников (…)».
            sim.funding = Some(format!(
                "перевод {} внутри симуляции (у кошелька {have}, нужно {})",
                dry_fund::text(&fs),
                io.need
            ));
        }
        calls.push(pc::balance(wallet, t, wallet));
    }
    let in_idx = io.token_in.map(|_| calls.len() - 1);
    // Баланс выхода до и после обмена; BNB без dex.multicall3 — выход не измеряется (только обмен и газ).
    let read_out = out_call(ctx, wallet, io.token_out);
    let before = read_out.clone().map(|c| {
        calls.push(c);
        calls.len() - 1
    });
    let mains = push_main(ctx, r, wallet, &mut calls, &mut values, &mut sim).await?;
    let after = read_out.map(|c| {
        calls.push(c);
        calls.len() - 1
    });
    dry_surplus::after_swap(&mut calls, wallet, &mut watch);
    // Исполнитель не развёрнут (режим «через исполнитель», решение 122): его код и владелец — подменой состояния.
    let mut over: Vec<_> = match &r.built {
        Built::Exec(x) => x.over.iter().cloned().collect(),
        _ => Vec::new(),
    };
    over.extend(io.credit.iter().filter(|_| credited).cloned());
    let res = simulate::run_anchored(ctx, &calls, &values, &over, r.at, parent).await?;
    // Решение 407: подмена достоверна, только если balanceOf входа после неё — ровно need.
    let seen = in_idx.and_then(|i| pc::u256(&res[i]));
    if credited && seen != Some(io.need) {
        sim.credit_bad = true;
        sim.reason = Some(format!(
            "подмена баланса недостоверна: balanceOf после подмены {seen:?}, подставлено {}",
            io.need
        ));
        return Ok(sim);
    }
    judge(r, &res, &mains, in_idx.map(|i| (i, io.need)), &mut sim);
    if sim.reason.is_none() {
        let read = |i: Option<usize>| i.and_then(|i| pc::u256(&res[i]));
        sim.out = read(before)
            .zip(read(after))
            .map(|(a, b)| b.saturating_sub(a));
        dry_surplus_cut::apply(ctx, r, &watch, &res, funded, &mut sim).await;
    }
    Ok(sim)
}

/// Чтение баланса выхода: токен — balanceOf; BNB — Multicall3.getEthBalance. None — BNB без dex.multicall3. Ошибок нет.
fn out_call(ctx: &super::super::ctx::Ctx, wallet: Address, token: Option<Address>) -> Option<Call> {
    match token {
        Some(t) => Some(pc::balance(wallet, t, wallet)),
        None => multicall::address(ctx).map(|mc| Call {
            from: wallet,
            to: mc,
            input: Bytes::from(IMulticall3Eth::getEthBalanceCall { addr: wallet }.abi_encode()),
        }),
    }
}

/// Вызовы обмена: (номер вызова, разрешение ли это). Одна транзакция — разрешения и execute с базой проверки баланса
/// по кошельку прямо сейчас (как exec/trade.rs::send_plan); бандл — разметка exec/bundle_sim.rs. Ошибки: узел (база).
async fn push_main(
    ctx: &super::super::ctx::Ctx,
    r: &Ready,
    wallet: Address,
    calls: &mut Vec<Call>,
    values: &mut Vec<(usize, U256)>,
    sim: &mut Sim,
) -> GatewayResult<Vec<(usize, bool)>> {
    let mut mains = Vec::new();
    match &r.built {
        Built::One(p) => {
            for (to, data) in &p.approvals {
                calls.push(Call {
                    from: wallet,
                    to: *to,
                    input: data.clone(),
                });
                mains.push((calls.len() - 1, true));
            }
            sim.approvals = p.approvals.len();
            let base = balances::token_units(ctx, p.token_out, wallet).await?;
            calls.push(Call {
                from: wallet,
                to: p.router,
                input: p.data(base),
            });
            if !p.value().is_zero() {
                values.push((calls.len() - 1, p.value()));
            }
            mains.push((calls.len() - 1, false));
        }
        Built::Exec(x) => {
            for (to, data) in &x.approvals {
                calls.push(Call {
                    from: wallet,
                    to: *to,
                    input: data.clone(),
                });
                mains.push((calls.len() - 1, true));
            }
            sim.approvals = x.approvals.len();
            calls.push(Call {
                from: wallet,
                to: x.call.executor,
                input: x.call.data(),
            });
            // Решение 272: продажа нативного BNB через executeNative — с value вызова.
            if !x.call.value().is_zero() {
                values.push((calls.len() - 1, x.call.value()));
            }
            mains.push((calls.len() - 1, false));
        }
        Built::Many(b) => {
            let s = build_sim(b, wallet);
            let shift = calls.len();
            sim.approvals = b.txs.iter().filter(|t| !t.is_swap()).count();
            for (i, &ci) in s.main.iter().enumerate() {
                mains.push((shift + ci, !b.txs[i].is_swap()));
            }
            values.extend(s.values.iter().map(|(i, v)| (shift + i, *v)));
            calls.extend(s.calls);
        }
    }
    Ok(mains)
}
