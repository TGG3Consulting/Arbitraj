//! Файл: venues/bsc_dex/check/native_probe_exec.rs
//! Что делает: решение 272 — проба нативной монеты BNB в пуле её обёртки WBNB через контракт-исполнитель v2
//! (executeNative), для пулов, через которые BNB идёт только исполнителем (exec/native_exec.rs::exec_ok: форки V2 / V3,
//! Algebra, Solidly, DODO, StableSwap, Maverick, Slipstream). Тот же замер, что у Universal Router (check/native_probe.rs:
//! газ обёртки сверх газа того же обмена токеном), но обе стороны сравнения — вызовы исполнителя кошелька проб, в ОДНОМ
//! симулированном блоке eth_simulateV1 от кошелька: USDT на кошелёк из другого пула → разрешения USDT и WBNB
//! исполнителю → покупка execute (USDT → WBNB, ERC-20) → покупка executeNative (USDT → BNB) → продажа execute
//! (WBNB → USDT) → продажа executeNative (value BNB → USDT) → нативный перевод на пустой адрес. Газ обёртки покупки =
//! газ executeNative − газ execute той же стороны (не меньше 0, check/native_probe.rs::extra), продажи — так же.
//! В каждом обмене — «кто меняет» через исполнитель (check/probe_exec.rs::emitter: событие самого пула).
//! Исполнитель не v2 / v3 — причина (котировка BNB через такой пул без замера — «не проверено», CoinNotChecked).
//! Кто вызывает: check/native_probe.rs (after).
//! От чего зависит: exec/executor/{mod, call, onchain, code}.rs, exec/chain.rs (Step), exec/plan.rs (Order),
//! wallet/approve_executor.rs, check/{simulate, sim_base, probe, probe_calls, probe_exec, native_probe}.rs.
//! Настройки: через вызываемые — dex.native_wrapped, dex.deadline_s, dex.approve_unlimited.
//!
//! Суммы: покупки — по ½ check.probe_usdt каждая; продажи и перевод — ¼ и ⅛ купленного обычной пробой (m.buy_got) —
//! не больше половины купленного каждой покупкой: проба меряет газ, а не цену.

use alloy_primitives::{Address, Bytes, U256, keccak256};

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::exec::chain::Step;
use super::super::exec::executor::{self, call, onchain};
use super::super::exec::native;
use super::super::exec::plan::Order;
use super::super::node::heads::Heads;
use super::super::pools::registry::Entry;
use super::super::wallet::approve_executor;
use super::native_probe::extra;
use super::probe::Measured;
use super::probe_calls as pc;
use super::simulate::{self, Call};
use super::{probe_exec, sim_base};

/// Симуляция. Принимает: пул (монета — обёртка BNB), источник USDT, кошелёк проб, сумму USDT, головы, обычную пробу.
/// Возвращает: Ok(Ok((газ обёртки покупки, газ обёртки продажи, газ нативного перевода))) или Ok(Err(причина)) —
/// исполнитель не v2 / v3 / вызов откатился / мерить не на чем. Ошибки: настройки, узел, сверка исполнителя (onchain.rs).
pub async fn run(
    ctx: &Ctx,
    e: &Entry,
    (source, wallet): (Address, Address),
    usdt_amount: U256,
    heads: &Heads,
    m: &Measured,
) -> GatewayResult<Result<(u64, u64, u64), String>> {
    let Some(x) = executor::address(ctx)? else {
        return Ok(Err("у кошелька проб не развёрнут исполнитель".into()));
    };
    let v = onchain::ensure(ctx, x, wallet).await?;
    if !v.native() {
        return Ok(Err(format!(
            "исполнитель {x:#x} — версия {}: executeNative нет, BNB через этот пул не идёт (переразверните исполнитель)",
            v.label()
        )));
    }
    let Some((_, w)) = native::wrapped(ctx) else {
        return Ok(Err(
            "не заданы dex.native_ticker / dex.native_wrapped".into()
        ));
    };
    let half = usdt_amount / U256::from(2u8);
    let (sell_amt, send_amt) = (m.buy_got / U256::from(4u8), m.buy_got / U256::from(8u8));
    if half.is_zero() || send_amt.is_zero() {
        return Ok(Err("куплено слишком мало для пробы".into()));
    }
    let (at, now) = sim_base::head(ctx, heads).await?;
    let one = |tin: Address, tout: Address, amount: U256| -> GatewayResult<call::ExecCall> {
        let step = Step {
            entry: e.clone(),
            order: Order {
                token_in: tin,
                token_out: tout,
                amount_in: amount,
                min_out: U256::from(1u8),
            },
            from_wallet: true,
            to_wallet: true,
        };
        call::build(ctx, &[vec![step]], x, now)
    };
    let buy = one(e.usdt, e.coin, half)?;
    let buy_n = one(e.usdt, e.coin, half)?.with_native(ctx, w, false)?;
    let sell = one(e.coin, e.usdt, sell_amt)?;
    let sell_n = one(e.coin, e.usdt, sell_amt)?.with_native(ctx, w, true)?;
    let mut list = vec![pc::transfer(source, e.usdt, wallet, usdt_amount)];
    let mut approvals = approve_executor::all(ctx, e.usdt, x, usdt_amount)?;
    approvals.extend(approve_executor::all(ctx, e.coin, x, sell_amt)?);
    list.extend(approvals.iter().map(|(to, d)| Call {
        from: wallet,
        to: *to,
        input: d.clone(),
    }));
    let b = list.len();
    let empty = Address::from_slice(&keccak256(wallet.as_slice())[12..]);
    for c in [&buy, &buy_n, &sell, &sell_n] {
        list.push(Call {
            from: wallet,
            to: x,
            input: c.data(),
        });
    }
    list.push(Call {
        from: wallet,
        to: empty,
        input: Bytes::new(),
    });
    let values = [(b + 3, sell_n.value()), (b + 4, send_amt)];
    let res = simulate::run_valued(ctx, &list, &values, at).await?;
    if let Some((i, r)) = res.iter().enumerate().find(|(_, r)| !r.ok()) {
        return Ok(Err(format!(
            "шаг {i} пробы через исполнитель откатился: {}",
            r.why()
        )));
    }
    for r in res.iter().skip(b).take(4) {
        if let Err(err) = probe_exec::emitter(ctx, e, &r.logs).await {
            return Ok(Err(err.message));
        }
    }
    let gas = |i: usize| res[i].gas_used.to::<u64>();
    Ok(Ok((
        extra(gas(b + 1), gas(b)),
        extra(gas(b + 3), gas(b + 2)),
        gas(b + 4),
    )))
}
