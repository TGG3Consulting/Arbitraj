//! Файл: venues/bsc_dex/check/probe.rs
//! Что делает: пробный обмен в любом пуле (V2, V3, V4) через eth_simulateV1 (решение 30) — налог на покупку,
//! продажу и перевод, газ обмена и газ разрешений в обе стороны. Тот же путь, что у настоящей сделки: у поддержанной
//! DEX — Permit2 + Universal Router с проверкой баланса (exec/plan.rs); у ФОРКА (family.fork, решения 59, 82) — его
//! собственный classic V2 Router + прямой ERC-20 approve (exec/fork_swap::probe_swap), иначе UR чужой пул не найдёт и
//! форк-монета с налогом не проверится; у вида без своего пути (family.tradable = false) — execute своего контракта-
//! исполнителя одним шагом этого пула (check/probe_exec.rs, решение 122 X). Выбор — part(). В одном симулированном блоке,
//! от имени кошелька бота (без подписи): USDT на кошелёк из другого пула → разрешения → расчёт без налога
//! (квотер DEX) → покупка → сколько монет пришло (налог покупки) → перевод четверти на пустой адрес (налог перевода) →
//! разрешения продажи → квотер на продажу остатка (расчёт без налога) → баланс встречного токена → продажа остатка →
//! баланс встречного токена (налог продажи = (квотер − прирост баланса) / квотер; решение 146: настоящей продажей,
//! а не переводом на адрес пула — токены с налогом «только при обмене через зарегистрированные пулы», живой случай
//! FLOKI 04.11, переводом не ловятся). Продажа откатилась или квотер не ответил — налог продажи НЕ ИЗМЕРЕН (не 0).
//! В покупке и продаже проверяется, что обмен сделал сам этот пул (check/emitter.rs, решение 59): Universal Router сам
//! выбирает пару по своей фабрике; через исполнитель — событие самого пула по адресу шага (check/probe_exec.rs::emitter).
//! Кто вызывает: check/taxes.rs.
//! От чего зависит: check/simulate.rs, check/sim_base.rs (время блока), check/probe_calls.rs, check/probe_exec.rs, exec/plan.rs, exec/fork_swap.rs,
//! quote/quoter_calls.rs, wallet/approve.rs, pools/registry.rs.
//! Настройки: через вызываемые (адреса квотера, роутеров, исполнителя; dex.deadline_s).
//!
//! Официального рецепта измерения налога нет (research/flow/bsc_dex.md, раздел 6). Квотер DEX выдаёт выход БЕЗ налога
//! токена (решение 146, проверено по коду): V2 Router.getAmountsOut — математика резервов, без переводов; QuoterV2
//! (Uniswap/PancakeSwap V3, Algebra, Slipstream, KyberSwap Elastic) и MaverickV2Quoter — откат в колбэке обмена ДО
//! перевода входного токена в пул; CLQuoter/BinQuoter Infinity и V4Quoter — откат с дельтами без переводов; DODO
//! querySell*, Solidly getAmountOut, StableSwap get_dy/calculateSwap, WooFi query — view-расчёт без переводов.
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
use super::probe_calls as pc;
use super::probe_exec;
use super::sim_base;
use super::simulate::{self, Call};

/// Почему налог продажи пула не измерен (решение 146).
#[derive(Debug, Clone, PartialEq)]
pub struct SellMiss {
    /// true — продажа пробы (или её разрешения) откатилась: вердикт по пулу; false — помеха (квотер не ответил,
    /// ответ не по ABI).
    pub reverted: bool,
    /// Причина текстом.
    pub why: String,
}

/// Что измерено (в минимальных единицах; проценты считает check/pool_taxes.rs).
#[derive(Debug, Clone)]
pub struct Measured {
    pub buy_expected: U256,
    pub buy_got: U256,
    /// Продажа (решение 146): Ok((ответ квотера на продажу, прирост встречного токена кошелька за продажу)) в
    /// единицах встречного токена пула (Entry.usdt); Err — налог продажи не измерен.
    pub sell: Result<(U256, U256), SellMiss>,
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
/// NotConfigured — обмен сделал не этот пул (пул исключён, check/emitter.rs); NoRoute — через исполнитель пул не выпустил
/// события или пробу через этот пул собрать нечем (check/probe_exec.rs).
pub async fn run(
    ctx: &Ctx,
    e: &Entry,
    source: Address,
    wallet: Address,
    usdt_amount: U256,
    heads: &Heads,
) -> GatewayResult<Result<Measured, String>> {
    let (usdt, coin) = (e.usdt, e.coin);
    let empty = Address::from_slice(&keccak256(wallet.as_slice())[12..]);
    // Обе симуляции — поверх одного блока `at` (номер, не «latest»), срок транзакции — от его времени (решение 61);
    // симулированный блок — время головы + 1 с (решение 127, check/sim_base.rs): у пробы есть срок (deadline роутера),
    // с +12 с узла BSC проба падала бы при dex.deadline_s ≤ 12.
    let (at, now) = sim_base::head(ctx, heads).await?;
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
    // Через исполнитель (tradable = false) — событие самого пула по адресу шага (check/probe_exec.rs::emitter).
    probe_exec::emitter(ctx, e, &first[x].logs).await?;
    let (Some(buy_expected), Some(bought)) = (
        quoter_calls::decode(e, &first[q].return_data),
        pc::u256(&first[x + 1]),
    ) else {
        return Ok(Err("ответ квотера или баланса не по ABI".into()));
    };
    // Доли купленного: четверть — пробный перевод, остаток (четверть) — продажа; половина остаётся на кошельке
    // (раньше уходила переводом на адрес пула; решение 146: у V2 такой перевод исказил бы продажу — пара считает вход
    // как balanceOf − резерв). Остаток — тот же, что раньше, газ продажи сравним с прежними замерами.
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
        pc::transfer(wallet, coin, empty, quarter),
        pc::balance(wallet, coin, empty),
    ]);
    list.extend(sell_aps.iter().map(|(to, d)| Call {
        from: wallet,
        to: *to,
        input: d.clone(),
    }));
    // Решение 146: квотер на продажу ТОГО ЖЕ остатка — прямо перед продажей (после переводов и разрешений, состояние
    // пула то же, что увидит продажа), баланс встречного токена кошелька до и после продажи — в том же пакете.
    let (sq_to, sq_data) = quoter_calls::request(ctx, e, coin, rest)?;
    let sq = list.len();
    list.push(Call {
        from: wallet,
        to: sq_to,
        input: sq_data,
    });
    list.push(pc::balance(wallet, usdt, wallet));
    list.push(Call {
        from: wallet,
        to: sell_router,
        input: sell_data,
    });
    list.push(pc::balance(wallet, usdt, wallet));
    let all = simulate::run(ctx, &list, at).await?;
    if let Some(r) = all.iter().find(|r| pc::late(r)) {
        let m = format!(
            "TransactionDeadlinePassed: срок транзакции вышел к симулированному блоку (время головы + 1 с, check/sim_base.rs) — проверьте dex.deadline_s и свежесть узла: {}",
            r.why()
        );
        return Err(ctx.err(ErrorCode::NoRoute, m));
    }
    if let Some((i, r)) = all.iter().enumerate().take(base + 2).find(|(_, r)| !r.ok()) {
        return Ok(Err(format!(
            "шаг {i} пробы (перевод монеты) откатился: {}",
            r.why()
        )));
    }
    let Some(xb) = pc::u256(&all[base + 1]) else {
        return Ok(Err("ответ баланса не по ABI".into()));
    };
    let sell_appr = &all[base + 2..sq];
    let sell_swap = &all[sq + 2];
    let sold = sell_swap.ok() && sell_appr.iter().all(|c| c.ok());
    if sold {
        probe_exec::emitter(ctx, e, &sell_swap.logs).await?;
    }
    let gas = |r: &[simulate::CallResult]| r.iter().map(|c| c.gas_used.to::<u64>()).sum::<u64>();
    Ok(Ok(Measured {
        buy_expected,
        buy_got: bought,
        sell: pc::sell_result(e, &all[sq..sq + 4], sold),
        transfer_sent: quarter,
        transfer_got: xb,
        gas_buy: all[x].gas_used.to(),
        gas_approve_buy: gas(&all[1..1 + n_appr]),
        gas_transfer: all[base].gas_used.to(),
        gas_sell: sold.then(|| (sell_swap.gas_used.to(), gas(sell_appr))),
    }))
}

/// Части свопа пробы (разрешения, роутер, calldata) по виду пула. Форк (family.fork, решения 59, 82) — свой classic
/// V2 Router и прямой ERC-20 approve (fork_swap::probe_swap), без Universal Router/Permit2: иначе проба форк-пула шла бы
/// через UR, который чужой пул не находит, и форк-монета с налогом не проверялась бы. Иначе — Universal Router своей
/// фабрики (exec/plan.rs), база BALANCE_CHECK = баланс выходного токена кошелька (решение 40). Вид без своего пути —
/// исполнитель (check/probe_exec.rs). Принимает: пул, ордер пробы (минимум 1), кошелёк, время блока.
/// Ошибки: NotConfigured (настройки/адреса), NoRoute (пробу через пул собрать нечем), узел (баланс для UR-базы).
async fn part(
    ctx: &Ctx,
    e: &Entry,
    order: Order,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<Approval>, Address, Bytes)> {
    // Решение 122 F/X: у вида без своего пути исполнения (family.tradable = false: DODO, THENA Fusion, Solidly,
    // Slipstream, форки V3, StableSwap, Maverick) пробу не собрать ни через Universal Router (чужой пул он не найдёт), ни
    // через classic V2 Router форка (у V3-форка его нет). Путь сделки для них — шаг контракта-исполнителя: тем же
    // вызовом execute идёт и проба (check/probe_exec.rs); нет шага / не задан адрес — точная причина оттуда.
    if !e.family.tradable {
        return probe_exec::part(ctx, e, order, wallet, now).await;
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
