//! Файл: venues/bsc_dex/check/probe.rs
//! Что делает: пробный обмен в любом пуле (V2, V3, V4) через eth_simulateV1 (решение 30) — налог на покупку,
//! продажу и перевод, газ обмена и газ разрешений в обе стороны. Тот же путь, что у настоящей сделки:
//! Permit2 + Universal Router своей DEX с проверкой баланса (exec/plan.rs). В одном симулированном блоке,
//! от имени кошелька бота (без подписи): USDT на кошелёк из другого пула → разрешения → расчёт без налога
//! (квотер DEX) → покупка → сколько монет пришло (налог покупки) → перевод половины на адрес, где лежат токены
//! пула (налог продажи: насколько вырос его баланс) → перевод четверти на пустой адрес (налог перевода) →
//! разрешения и продажа остатка (газ продажи). В покупке и продаже проверяется, что обмен сделал сам этот пул
//! (check/emitter.rs, решение 59): Universal Router сам выбирает пару по своей фабрике.
//! Кто вызывает: check/taxes.rs.
//! От чего зависит: check/simulate.rs, check/probe_calls.rs, check/emitter.rs, exec/plan.rs, quote/quoter_calls.rs, pools/registry.rs.
//! Настройки: через вызываемые; dex.vault [Infinity] — где лежат токены пулов Infinity.
//!
//! Официального рецепта измерения налога нет (research/flow/bsc_dex.md, раздел 6). Токены V2/V3 лежат в самом пуле,
//! Infinity — в Vault («Vault … holds all tokens», research/flow/bsc_v4.md, раздел 1), Uniswap V4 — в PoolManager.
//! Пустой адрес — последние 20 байт keccak256(адрес кошелька) — вычисляется, не зашит.
//! Ограничение (подтверждено техлидом, Issue #7): налог перевода меряется на пустой адрес, а не на депозит биржи —
//! монета, у которой налог зависит от получателя (белые/чёрные списки), может взять на депозит другой налог;
//! факт перевода сверяется по событиям Transfer (exec/fill.rs).
//! Минимум выхода пробы — 1 минимальная единица токена: проба меряет налог и не должна откатываться из-за минимума,
//! а минимум 0 запрещён (exec/plan.rs). Проба только симулируется (eth_simulateV1), в сеть не уходит.

use alloy_primitives::{Address, U256, keccak256};

use crate::core::types::{PoolKind, PoolRef};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::plan::{self, Order};
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;
use super::super::quote::quoter_calls;
use super::super::wallet::balances;
use super::emitter;
use super::probe_calls as pc;
use super::simulate::{self, Call};

/// Что измерено (в минимальных единицах; проценты считает taxes.rs).
#[derive(Debug, Clone)]
pub struct Measured {
    pub buy_expected: U256,
    pub buy_got: U256,
    pub sell_sent: U256,
    pub sell_got: U256,
    pub transfer_sent: U256,
    pub transfer_got: U256,
    pub gas_buy: u64,
    pub gas_approve_buy: u64,
    /// Газ перевода монеты на другой адрес (пробный перевод налога перевода).
    pub gas_transfer: u64,
    /// Газ продажи и её разрешений; None — продажа откатилась.
    pub gas_sell: Option<(u64, u64)>,
}

/// Пробный обмен. Принимает: пул, источник USDT (другой пул), кошелёк, сумму USDT, время блока.
/// Возвращает: Ok(Measured) или Ok(Err(причина)); Err — настройки, узел (метод не поддерживается и т.п.),
/// NotConfigured — обмен сделал не этот пул (пул исключён, check/emitter.rs).
pub async fn run(
    ctx: &Ctx,
    e: &Entry,
    source: Address,
    wallet: Address,
    usdt_amount: U256,
    now: u64,
) -> GatewayResult<Result<Measured, String>> {
    let (usdt, coin) = (e.usdt, e.coin);
    let holder = holder(ctx, e)?;
    let empty = Address::from_slice(&keccak256(wallet.as_slice())[12..]);
    let buy = plan::build(
        ctx,
        &plan::single(
            e,
            Order {
                token_in: usdt,
                token_out: coin,
                amount_in: usdt_amount,
                min_out: U256::from(1u8),
            },
        ),
        wallet,
        now,
        true,
    )
    .await?;
    let (qto, qdata) = quoter_calls::request(ctx, e, usdt, usdt_amount)?;
    let mut list = vec![pc::transfer(source, usdt, wallet, usdt_amount)];
    list.extend(buy.approvals.iter().map(|(to, d)| Call {
        from: wallet,
        to: *to,
        input: d.clone(),
    }));
    let n_appr = buy.approvals.len();
    list.push(Call {
        from: wallet,
        to: qto,
        input: qdata,
    });
    list.push(Call {
        from: wallet,
        to: buy.router,
        input: buy.data(balances::token_units(ctx, coin, wallet).await?),
    });
    list.push(pc::balance(wallet, coin, wallet));
    let first = simulate::run(ctx, &list).await?;
    let (q, x) = (1 + n_appr, 2 + n_appr);
    if let Some((i, r)) = first.iter().enumerate().find(|(_, r)| !r.ok()) {
        return pc::buy_revert(ctx, i, r, q).map(Err);
    }
    // Кто меняет (решение 59): обмен должен сделать сам этот пул — иначе пул исключён, проверка «не проверено».
    if let Err(why) = emitter::check(ctx, &[e], &first[x].logs).await {
        return Err(ctx.err(ErrorCode::NotConfigured, why));
    }
    let (Some(buy_expected), Some(bought)) = (
        quoter_calls::decode(e, &first[q].return_data),
        pc::u256(&first[x + 1]),
    ) else {
        return Ok(Err("ответ квотера или баланса не по ABI".into()));
    };
    // База USDT для проверки продажи: в симуляции USDT источника уже потрачен на покупку — остаётся реальный баланс.
    let usdt_base = balances::token_units(ctx, usdt, wallet).await?;
    let (half, quarter) = (bought / U256::from(2u8), bought / U256::from(4u8));
    let rest = bought - half - quarter;
    let sell = plan::build(
        ctx,
        &plan::single(
            e,
            Order {
                token_in: coin,
                token_out: usdt,
                amount_in: rest,
                min_out: U256::from(1u8),
            },
        ),
        wallet,
        now,
        true,
    )
    .await?;
    let base = list.len();
    list.extend([
        pc::balance(wallet, coin, holder),
        pc::transfer(wallet, coin, holder, half),
        pc::balance(wallet, coin, holder),
        pc::transfer(wallet, coin, empty, quarter),
        pc::balance(wallet, coin, empty),
    ]);
    list.extend(sell.approvals.iter().map(|(to, d)| Call {
        from: wallet,
        to: *to,
        input: d.clone(),
    }));
    list.push(Call {
        from: wallet,
        to: sell.router,
        input: sell.data(usdt_base),
    });
    let all = simulate::run(ctx, &list).await?;
    if let Some((i, r)) = all.iter().enumerate().take(base + 5).find(|(_, r)| !r.ok()) {
        return Ok(Err(format!(
            "шаг {i} пробы (перевод монеты) откатился: {}",
            r.why()
        )));
    }
    let (Some(pb0), Some(pb1), Some(xb)) = (
        pc::u256(&all[base]),
        pc::u256(&all[base + 2]),
        pc::u256(&all[base + 4]),
    ) else {
        return Ok(Err("ответ баланса не по ABI".into()));
    };
    if sell_ok(&all)
        && let Err(why) = emitter::check(ctx, &[e], &all[all.len() - 1].logs).await
    {
        return Err(ctx.err(ErrorCode::NotConfigured, why));
    }
    let gas = |r: &[simulate::CallResult]| r.iter().map(|c| c.gas_used.to::<u64>()).sum::<u64>();
    let sell_appr = &all[base + 5..all.len() - 1];
    let sell_swap = &all[all.len() - 1];
    Ok(Ok(Measured {
        buy_expected,
        buy_got: bought,
        sell_sent: half,
        sell_got: pb1.saturating_sub(pb0),
        transfer_sent: quarter,
        transfer_got: xb,
        gas_buy: all[x].gas_used.to(),
        gas_approve_buy: gas(&all[1..1 + n_appr]),
        gas_transfer: all[base + 3].gas_used.to(),
        gas_sell: (sell_swap.ok() && sell_appr.iter().all(|c| c.ok()))
            .then(|| (sell_swap.gas_used.to(), gas(sell_appr))),
    }))
}

/// Продажа пробы прошла (последний вызов). Ошибок нет.
fn sell_ok(all: &[simulate::CallResult]) -> bool {
    all.last().is_some_and(|c| c.ok())
}

/// Адрес, где лежат токены пула: V2/V3 — сам пул; Infinity — Vault (dex.vault); Uniswap V4 — PoolManager.
/// Ошибки: NotConfigured (dex.vault).
fn holder(ctx: &Ctx, e: &Entry) -> GatewayResult<Address> {
    Ok(match (&e.pool.reference, e.pool.kind, e.family.proto) {
        (PoolRef::Contract(a), PoolKind::V2 | PoolKind::V3, _) => *a,
        (_, _, Proto::InfinityCl | Proto::InfinityBin) => ctx.dex_addr("dex.vault", e.family.id)?,
        (PoolRef::Singleton { manager, .. }, _, _) => *manager,
        (PoolRef::Contract(a), _, _) => *a,
    })
}
