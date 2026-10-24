//! Файл: venues/bsc_dex/check/native_probe.rs
//! Что делает: проба нативной монеты BNB (решение 62) в пуле её обёртки WBNB — после обычной пробы (check/probe.rs:
//! налоги WBNB как ERC-20 — стандартной проверкой, без исключений в вердиктах). В одном симулированном блоке
//! eth_simulateV1 от имени кошелька тем же путём, что сделка (exec/native.rs, exec/plan.rs): USDT на кошелёк из
//! другого пула → разрешения → покупка с UNWRAP_WETH на кошелёк → продажа половины купленного с value и WRAP_ETH →
//! нативный перевод четверти на пустой адрес. Меряется: газ обёртки сверх газа того же обмена токеном (покупка,
//! продажа) — затрата WrapGas котировки; газ нативного перевода — стоимость вывода на биржу. В обмене проверяется
//! «кто меняет» (check/emitter.rs, решение 59).
//! Налога у нативного перевода нет: это не вызов контракта токена (value транзакции), налог взять некому.
//! Кто вызывает: check/taxes.rs (после удачной пробы пула нативной монеты).
//! От чего зависит: check/simulate.rs, check/probe.rs (Measured), check/probe_calls.rs, check/emitter.rs,
//! check/state.rs, exec/plan.rs, exec/native.rs, wallet/balances.rs, pools/registry.rs.
//! Настройки: через вызываемые — dex.native_ticker, dex.native_wrapped, адреса Universal Router и Permit2.
//!
//! Сумма покупки — та же check.probe_usdt, что у обычной пробы; продажа и перевод — доли купленного в обычной пробе
//! (то же состояние блока, у WBNB налога нет). Пустой адрес — как в check/probe.rs. Газ — gasUsed вызова
//! (execution-apis execute.yaml, CallResult.gasUsed); «сверх» — разность с обычной пробой того же пула, не меньше 0.

use alloy_primitives::{Address, Bytes, U256, keccak256};

use crate::core::types::{Asset, Side};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::exec::native;
use super::super::node::heads::Heads;
use super::super::exec::plan::{self, Order, Plan};
use super::super::pools::registry::Entry;
use super::super::wallet::balances;
use super::emitter;
use super::probe::Measured;
use super::probe_calls as pc;
use super::simulate::{self, Call};
use super::state::Checks;

/// Проба нативной монеты для пула и запись итога в checks (PoolFacts::wrap_gas, газ нативного перевода); монета пула
/// не нативная — ничего. Принимает: пул, (источник USDT, кошелёк), сумму USDT пробы, головы сети, итог обычной пробы.
/// Ошибок нет: сбой — Some(Err(причина)) в фактах пула и запись в журнал; котировка BNB без замера — «не проверено».
pub async fn after(
    ctx: &Ctx,
    checks: &Checks,
    e: &Entry,
    (source, wallet): (Address, Address),
    usdt_amount: U256,
    heads: &Heads,
    m: &Measured,
) {
    let asset = Asset {
        ticker: e.ticker.clone(),
        bsc_contract: Some(e.coin),
    };
    // Только V2/V3 (exec/native.rs::pool_ok): через V4 / Infinity BNB не торгуется — и мерить нечего.
    if !native::is_native(ctx, &asset) || !native::pool_ok(e.pool.kind) {
        return;
    }
    let res = match run(ctx, e, source, wallet, usdt_amount, heads, m).await {
        Ok(Ok((buy, sell, transfer))) => {
            checks.set_native_transfer_gas(&e.ticker, transfer);
            Ok((buy, sell))
        }
        Ok(Err(why)) => Err(why),
        Err(err) => Err(err.message),
    };
    if let Err(why) = &res {
        let msg = format!(
            "{} {}: проба нативной монеты (WRAP/UNWRAP) не прошла — {why}",
            e.family.title, e.ticker
        );
        ctx.log(Level::Warn, "dex.native_probe", msg, None).await;
    }
    checks.update_pool(&e.pool.reference, |f| f.wrap_gas = Some(res));
}

/// Симуляция. Возвращает: Ok(Ok((газ сверх покупки, газ сверх продажи, газ перевода))), Ok(Err(причина)) — вызов
/// откатился или обычная продажа не измерена. Ошибки: настройки, узел, «кто меняет».
async fn run(
    ctx: &Ctx,
    e: &Entry,
    source: Address,
    wallet: Address,
    usdt_amount: U256,
    heads: &Heads,
    m: &Measured,
) -> GatewayResult<Result<(u64, u64, u64), String>> {
    let Some((sell_erc20, _)) = m.gas_sell else {
        return Ok(Err(
            "обычная продажа пробы не прошла — сравнить не с чем".into()
        ));
    };
    // Доли ½ (продажа) и ¼ (перевод) купленного — те же, что в обычной пробе (check/probe.rs: half и quarter
    // после покупки): проба меряет газ, а не цену, и не должна упираться в остаток.
    let (sell_amt, send_amt) = (m.buy_got / U256::from(2u8), m.buy_got / U256::from(4u8));
    if send_amt.is_zero() {
        return Ok(Err("куплено слишком мало для пробы".into()));
    }
    // Поверх одного блока `at`, срок транзакции — от его времени (как в обычной пробе, check/probe.rs, решение 61).
    let (at, now) = heads.at(ctx).await?;
    let buy = side_plan(ctx, e, Side::Buy, usdt_amount, wallet, now).await?;
    let sell = side_plan(ctx, e, Side::Sell, sell_amt, wallet, now).await?;
    let mut list = vec![pc::transfer(source, e.usdt, wallet, usdt_amount)];
    list.extend(buy.approvals.iter().map(|(to, d)| Call {
        from: wallet,
        to: *to,
        input: d.clone(),
    }));
    let b = list.len();
    let usdt_base = balances::token_units(ctx, e.usdt, wallet).await?;
    let empty = Address::from_slice(&keccak256(wallet.as_slice())[12..]);
    list.extend([
        Call {
            from: wallet,
            to: buy.router,
            input: buy.data(U256::ZERO),
        },
        Call {
            from: wallet,
            to: sell.router,
            input: sell.data(usdt_base),
        },
        Call {
            from: wallet,
            to: empty,
            input: Bytes::new(),
        },
    ]);
    let values = [(b + 1, sell.value()), (b + 2, send_amt)];
    let res = simulate::run_valued(ctx, &list, &values, at).await?;
    if let Some((i, r)) = res.iter().enumerate().find(|(_, r)| !r.ok()) {
        return Ok(Err(format!("шаг {i} пробы откатился: {}", r.why())));
    }
    for i in [b, b + 1] {
        if let Err(why) = emitter::check(ctx, &[e], &res[i].logs).await {
            return Ok(Err(why));
        }
    }
    let gas = |i: usize| res[i].gas_used.to::<u64>();
    Ok(Ok((
        extra(gas(b), m.gas_buy),
        extra(gas(b + 1), sell_erc20),
        gas(b + 2),
    )))
}

/// Газ обёртки сверх обмена токеном: газ обмена с WRAP/UNWRAP − газ того же обмена токеном WBNB. Разность меньше 0
/// (обмен с обёрткой дешевле — например, нет записи баланса WBNB кошелька) — 0: обмен в котировке уже посчитан по
/// газу обмена токеном, он выше настоящего, и затрата завышается — это безопасная сторона (решение 62). Ошибок нет.
pub fn extra(native: u64, erc20: u64) -> u64 {
    native.saturating_sub(erc20)
}

/// План одной стороны с нативной монетой (как в сделке: exec/native.rs::adapt). Минимум — 1 единица (проба меряет газ,
/// минимум 0 запрещён). Ошибки: как у plan::build и native::adapt.
async fn side_plan(
    ctx: &Ctx,
    e: &Entry,
    side: Side,
    amount: U256,
    wallet: Address,
    now: u64,
) -> GatewayResult<Plan> {
    let (tin, tout) = match side {
        Side::Buy => (e.usdt, e.coin),
        Side::Sell => (e.coin, e.usdt),
    };
    let order = Order {
        token_in: tin,
        token_out: tout,
        amount_in: amount,
        // Минимум 1 единица — как в обычной пробе (check/probe.rs, шапка: минимум 0 запрещён exec/plan.rs).
        min_out: U256::from(1u8),
    };
    let mut legs = vec![plan::single(e, order)];
    let wrap = native::adapt(ctx, side, &mut legs)?;
    Ok(plan::build(ctx, &legs[0], wallet, now, true)
        .await?
        .with_native(wrap))
}
