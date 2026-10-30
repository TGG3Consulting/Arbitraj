//! Файл: venues/bsc_dex/check/probe.rs
//! Что делает: пробный обмен в любом пуле (V2, V3, V4) через eth_simulateV1 (решение 30) — налог на покупку,
//! продажу и перевод, газ обмена и газ разрешений в обе стороны. Тот же путь, что у настоящей сделки: у поддержанной
//! DEX — Permit2 + Universal Router с проверкой баланса (exec/plan.rs); у ФОРКА (family.fork, решения 59, 82) — его
//! собственный classic V2 Router + прямой ERC-20 approve (exec/fork_swap::probe_swap), иначе UR чужой пул не найдёт и
//! форк-монета с налогом не проверится. Выбор — part(). В одном симулированном блоке,
//! от имени кошелька бота (без подписи): USDT на кошелёк из другого пула → разрешения → расчёт без налога
//! (квотер DEX) → покупка → сколько монет пришло (налог покупки) → перевод половины на адрес, где лежат токены
//! пула (налог продажи: насколько вырос его баланс) → перевод четверти на пустой адрес (налог перевода) →
//! разрешения и продажа остатка (газ продажи). В покупке и продаже проверяется, что обмен сделал сам этот пул
//! (check/emitter.rs, решение 59): Universal Router сам выбирает пару по своей фабрике.
//! Кто вызывает: check/taxes.rs.
//! От чего зависит: check/simulate.rs, check/probe_calls.rs, check/emitter.rs, exec/plan.rs, exec/fork_swap.rs,
//! quote/quoter_calls.rs, wallet/approve.rs, pools/registry.rs.
//! Настройки: через вызываемые; dex.vault [Infinity] — где лежат токены пулов Infinity.
//!
//! Официального рецепта измерения налога нет (research/flow/bsc_dex.md, раздел 6). Токены V2/V3 лежат в самом пуле,
//! Infinity — в Vault («Vault … holds all tokens», research/flow/bsc_v4.md, раздел 1), Uniswap V4 — в PoolManager.
//! Пустой адрес — последние 20 байт keccak256(адрес кошелька) — вычисляется, не зашит.
//! Ограничение (подтверждено техлидом, Issue #7): налог перевода меряется на пустой адрес, а не на депозит биржи —
//! налог может зависеть от получателя (белые/чёрные списки); факт перевода — по событиям Transfer (exec/fill.rs).
//! Минимум выхода пробы — 1 минимальная единица токена: проба меряет налог и не должна откатываться из-за минимума,
//! а минимум 0 запрещён (exec/plan.rs). Проба только симулируется (eth_simulateV1), в сеть не уходит.

use alloy_primitives::{Address, Bytes, U256, keccak256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::fork_swap;
use super::super::exec::plan::{self, Order};
use super::super::node::heads::Heads;
use super::super::pools::registry::Entry;
use super::super::quote::quoter_calls;
use super::super::wallet::approve::Approval;
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

/// Пробный обмен. Принимает: пул, источник USDT (другой пул), кошелёк, сумму USDT, головы сети (блок, поверх которого
/// идут обе симуляции, и его время — для срока транзакции).
/// Возвращает: Ok(Measured) или Ok(Err(причина)); Err — настройки, узел (метод не поддерживается и т.п.),
/// NotConfigured — обмен сделал не этот пул (пул исключён, check/emitter.rs).
pub async fn run(
    ctx: &Ctx,
    e: &Entry,
    source: Address,
    wallet: Address,
    usdt_amount: U256,
    heads: &Heads,
) -> GatewayResult<Result<Measured, String>> {
    let (usdt, coin) = (e.usdt, e.coin);
    let holder = pc::holder(ctx, e)?;
    let empty = Address::from_slice(&keccak256(wallet.as_slice())[12..]);
    // Обе симуляции — поверх одного блока `at` (номер, не «latest»), срок транзакции — от его времени (решение 61).
    let (at, now) = heads.at(ctx).await?;
    let (buy_aps, buy_router, buy_data) = part(
        ctx,
        e,
        Order {
            token_in: usdt,
            token_out: coin,
            amount_in: usdt_amount,
            min_out: U256::from(1u8),
        },
        wallet,
        now,
    )
    .await?;
    let (qto, qdata) = quoter_calls::request(ctx, e, usdt, usdt_amount)?;
    let mut list = vec![pc::transfer(source, usdt, wallet, usdt_amount)];
    list.extend(buy_aps.iter().map(|(to, d)| Call {
        from: wallet,
        to: *to,
        input: d.clone(),
    }));
    let n_appr = buy_aps.len();
    list.push(Call {
        from: wallet,
        to: qto,
        input: qdata,
    });
    list.push(Call {
        from: wallet,
        to: buy_router,
        input: buy_data,
    });
    list.push(pc::balance(wallet, coin, wallet));
    let first = simulate::run(ctx, &list, at).await?;
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
    let (half, quarter) = (bought / U256::from(2u8), bought / U256::from(4u8));
    let rest = bought - half - quarter;
    let (sell_aps, sell_router, sell_data) = part(
        ctx,
        e,
        Order {
            token_in: coin,
            token_out: usdt,
            amount_in: rest,
            min_out: U256::from(1u8),
        },
        wallet,
        now,
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
    list.extend(sell_aps.iter().map(|(to, d)| Call {
        from: wallet,
        to: *to,
        input: d.clone(),
    }));
    list.push(Call {
        from: wallet,
        to: sell_router,
        input: sell_data,
    });
    let all = simulate::run(ctx, &list, at).await?;
    if let Some(r) = all.iter().find(|r| pc::late(r)) {
        let m = format!(
            "TransactionDeadlinePassed: срок транзакции меньше шага времени симулированного блока (у BSC +12 с к базовому, check/simulate.rs) — увеличьте dex.deadline_s: {}",
            r.why()
        );
        return Err(ctx.err(ErrorCode::NoRoute, m));
    }
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
    if pc::sell_ok(&all)
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

/// Части свопа пробы (разрешения, роутер, calldata) по виду пула. Форк (family.fork, решения 59, 82) — свой classic
/// V2 Router и прямой ERC-20 approve (fork_swap::probe_swap), без Universal Router/Permit2: иначе проба форк-пула шла бы
/// через UR, который чужой пул не находит, и форк-монета с налогом не проверялась бы. Иначе — Universal Router своей
/// фабрики (exec/plan.rs), база BALANCE_CHECK = баланс выходного токена кошелька (решение 40). Принимает: пул, ордер
/// пробы (минимум 1), кошелёк, время блока. Ошибки: NotConfigured (настройки/адреса), узел (баланс для UR-базы).
async fn part(
    ctx: &Ctx,
    e: &Entry,
    order: Order,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<Approval>, Address, Bytes)> {
    // DODO PMM (решение 86b): исполнения свопа пока нет (роутер DODO — фаза 86c), поэтому пробу налога через его
    // пул собрать нечем (ни Universal Router, ни свой роутер-форк). Чёткая причина, не молчаливый сбой: налог монеты
    // проверяется по её другим пулам (Pancake и т. п.), если они есть.
    if e.family.proto.is_dodo() {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            format!(
                "{}: проба налога через пул DODO невозможна — исполнение свопа ещё не реализовано (фаза 86c)",
                e.family.title
            ),
        ));
    }
    if e.family.fork {
        fork_swap::probe_swap(
            ctx,
            e,
            order.token_in,
            order.token_out,
            order.amount_in,
            wallet,
            now,
        )
    } else {
        let base = balances::token_units(ctx, order.token_out, wallet).await?;
        let plan = plan::build(ctx, &plan::single(e, order), wallet, now, true).await?;
        let data = plan.data(base);
        Ok((plan.approvals, plan.router, data))
    }
}
