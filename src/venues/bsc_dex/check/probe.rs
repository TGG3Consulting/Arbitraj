//! Файл: venues/bsc_dex/check/probe.rs
//! Что делает: пробный обмен в любом пуле (V2, V3, V4) через eth_simulateV1 (решение 30) — налог на покупку,
//! продажу и перевод, газ обмена и газ разрешений в обе стороны. Тот же путь, что у настоящей сделки: у поддержанной
//! DEX — Permit2 + Universal Router с проверкой баланса (exec/plan.rs); у ФОРКА (family.fork, решения 59, 82) — его
//! собственный classic V2 Router + прямой ERC-20 approve (exec/fork_swap::probe_swap), иначе UR чужой пул не найдёт и
//! форк-монета с налогом не проверится; у вида без своего пути (family.tradable = false) — execute своего контракта-
//! исполнителя одним шагом этого пула (check/probe_exec.rs, решение 122 X). Выбор — part(). В одном симулированном блоке,
//! от имени кошелька бота (без подписи): USDT на кошелёк из другого пула → разрешения → расчёт без налога
//! (квотер DEX) → покупка → сколько монет пришло (налог покупки) → разрешения продажи → квотер на продажу остатка
//! (расчёт без налога) → баланс встречного токена → продажа остатка → баланс встречного токена (налог продажи =
//! (квотер − прирост баланса) / квотер; решение 146: настоящей продажей, а не переводом на адрес пула — токены с налогом
//! «только при обмене через зарегистрированные пулы», живой случай FLOKI 04.11, переводом не ловятся) → перевод четверти
//! на пустой адрес (налог перевода). Решение 252: перевод — ПОСЛЕ продажи, как у настоящей продажи (перед ней перевода
//! монеты нет): у монеты с отражением (reflection) перевод перед продажей поднимает balanceOf пары выше резерва, и вход
//! продажи в паре V2 (balanceOf − резерв) больше настоящего (живые случаи PIG, SMARS 09–10.11, docs/notes/
//! reflection-tax-2026-11-10.md). Продажа откатилась или квотер не ответил — налог продажи НЕ ИЗМЕРЕН (не 0).
//! В покупке и продаже проверяется, что обмен сделал сам этот пул (check/emitter.rs, решение 59): Universal Router сам
//! выбирает пару по своей фабрике; через исполнитель — событие самого пула по адресу шага (check/probe_exec.rs::emitter).
//! Кошелёк пробы — кошелёк окружения (строка «Пробы» или, при перепроверке после отката продажи выхода, кошелёк сделки —
//! check/recheck.rs coins_by, дополнение 10.11 к решениям 247/252); купленное — прирост баланса монеты за покупку
//! (check/probe_buy.rs), а не весь баланс: на кошельке сделки могут лежать её монеты.
//! Кто вызывает: check/taxes.rs.
//! От чего зависит: check/simulate.rs, check/sim_base.rs (время блока), check/probe_calls.rs, check/probe_buy.rs, check/probe_exec.rs, exec/plan.rs, exec/fork_swap.rs,
//! quote/v2_pair_probe.rs (квотер DEX или, у V2-форка без роутера, резервы пары — решение 280), quote/probe_quote.rs
//! (у DEX с dex.quoter_virtual — виртуальный квотер с подменой кода в тех же симуляциях, решение 388), wallet/approve.rs, pools/registry.rs.
//! Настройки: через вызываемые (адреса квотера, роутеров, исполнителя; dex.deadline_s; dex.quoter_virtual [DEX]);
//! наименьший выход мелкого пула (решение 393) считает вызывающий (check/taxes.rs, dex.quoter_max_diff_pct).
//!
//! Официального рецепта измерения налога нет (research/flow/bsc_dex.md, раздел 6). Квотер DEX выдаёт выход БЕЗ налога
//! токена (решение 146, проверено по коду): V2 Router.getAmountsOut — математика резервов, без переводов; QuoterV2
//! (Uniswap/PancakeSwap V3, Algebra, Slipstream, KyberSwap Elastic) и MaverickV2Quoter — откат в колбэке обмена ДО
//! перевода входного токена в пул; CLQuoter/BinQuoter Infinity и V4Quoter — откат с дельтами без переводов; DODO
//! querySell*, Solidly getAmountOut, StableSwap get_dy/calculateSwap, WooFi query — view-расчёт без переводов.
//! Виртуальный квотер (решения 186, 388: DEX с dex.quoter_virtual — Metric, Algebra Integral, iZiSwap / Biswap V3,
//! SmarDex и др.) — swap самого пула с откатом в обратном вызове ДО перевода входа, тоже без налога (quote/probe_quote.rs).
//! Пустой адрес — последние 20 байт keccak256(адрес кошелька) — вычисляется, не зашит.
//! Ограничение (подтверждено техлидом, Issue #7): налог перевода меряется на пустой адрес, а не на депозит биржи —
//! налог может зависеть от получателя (белые/чёрные списки); факт перевода — по событиям Transfer (exec/fill.rs).
//! Решение 393: у МЕЛКОГО пула ответ квотера (покупки или продажи) меньше наименьшего выхода quote/side_fit.rs min_units —
//! налог не измерен и откат не вердикт (check/probe_tiny.rs); у глубокого пула — как до решения 393 (ревью t178, Н3).
//! Минимум выхода пробы — 1 минимальная единица токена: проба меряет налог и не должна откатываться из-за минимума,
//! а минимум 0 запрещён (exec/plan.rs). Проба только симулируется (eth_simulateV1), в сеть не уходит.

use alloy_primitives::{Address, Bytes, U256, keccak256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::fork_swap;
use super::super::exec::plan::{self, Order};
use super::super::node::heads::Heads;
use super::super::pools::registry::Entry;
use super::super::quote::{probe_quote, v2_pair_probe};
use super::super::wallet::approve::Approval;
use super::super::wallet::balances;
use super::probe_buy;
use super::probe_calls as pc;
use super::probe_exec;
use super::probe_tiny;
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
/// идут обе симуляции, и его время — для срока транзакции), наименьший выход для замера (решение 393: Some — мелкий
/// пул, check/probe_tiny.rs; None — глубокий пул, как до решения 393, без dex.quoter_max_diff_pct).
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
    min: Option<U256>,
) -> GatewayResult<Result<Measured, String>> {
    let (usdt, coin) = (e.usdt, e.coin);
    let empty = Address::from_slice(&keccak256(wallet.as_slice())[12..]);
    // Обе симуляции — поверх одного блока `at` (номер, не «latest»), срок транзакции — от его времени (решение 61);
    // симулированный блок — время головы + 1 с (решение 127, check/sim_base.rs): у пробы есть срок (deadline роутера),
    // с +12 с узла BSC проба падала бы при dex.deadline_s ≤ 12.
    let (at, now) = sim_base::head(ctx, heads).await?;
    let vista = if super::super::quote::vista_pricing::native_router(e) || (super::super::quote::ekubo_v2_pricing::enabled(e) && matches!(&e.pool.state,crate::core::types::PoolState::Ekubo(d) if d.native.is_some())) {
        Box::pin(super::vista_probe::Probe::of(ctx, e, at.number, wallet))
            .await?
            .map(Box::new)
    } else {
        None
    };
    let verified = vista.as_ref().map(|p| {
        let mut v = Box::new(e.clone());
        v.pool.state = p.state();
        v.pool.block = at.number;
        v
    });
    let e = verified.as_deref().unwrap_or(e);
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
        vista.as_deref(),
    )
    .await?;
    // Решение 388 (t174): «ожидалось без налога» — у DEX с виртуальным квотером его вызов с подменой кода в этих же
    // симуляциях (quote/probe_quote.rs); способ выбран один раз — вызов и разбор одного квотера.
    let way = probe_quote::Way::of(ctx, e);
    let over = way.overrides(ctx)?;
    let (qto, qdata) = way.request(ctx, e, usdt, usdt_amount)?;
    let mut list = vec![pc::transfer(source, usdt, wallet, usdt_amount)];
    if let Some(seed) = vista
        .as_ref()
        .and_then(|v| v.fund(wallet, usdt, usdt_amount))
    {
        list.push(seed);
    }
    let approvals_start = list.len();
    list.extend(buy_aps.iter().map(|(to, d)| Call {
        from: wallet,
        to: *to,
        input: d.clone(),
    }));
    let n_appr = buy_aps.len();
    // Дополнение 10.11 к решениям 247/252 (перепроверка кошельком сделки, на нём могут лежать монеты сделки):
    // купленное — прирост баланса монеты кошелька за покупку (баланс до и после в той же симуляции, probe_buy.rs).
    let h = probe_buy::buy_head(
        &mut list,
        (wallet, coin),
        (qto, qdata),
        (buy_router, buy_data),
    );
    let mut values = Vec::new();
    if let Some(v) = &vista {
        values.push((h.buy, v.value(usdt, usdt_amount)));
        if let Some(b) = v.balance(wallet, coin) {
            list[h.before] = b.clone();
            list[h.after] = b;
        }
    }
    let hash = vista.as_ref().map(|v| v.head.hash);
    let first = simulate::run_anchored(ctx, &list, &values, &over, at, hash).await?;
    let (q, x) = (h.quote, h.buy);
    if let Some((i, r)) = first.iter().enumerate().find(|(_, r)| !r.ok()) {
        // Решение 280: V2-форк без роутера — откат покупки через исполнитель — свойство пула, не монеты.
        if let Some(why) = v2_pair_probe::buy_unmeasured(e, i, q, r) {
            return Err(ctx.err(ErrorCode::SwapReverted, why));
        }
        // Ревью 388 (Н1): виртуальный квотер откатился, покупка прошла — помеха измерения, не вердикт по монете.
        if let Some(err) = way.quote_reverted(ctx, e, (i, q), r, first.get(x)) {
            return Err(err);
        }
        // Решение 393: откат покупки при крошечном ответе квотера — свойство суммы, не монеты.
        if let Some(min) = min
            && let Some(err) =
                probe_tiny::buy_revert(ctx, e, way, (i, q), &first, (usdt, usdt_amount), min)
        {
            return Err(err);
        }
        return pc::buy_revert(ctx, i, r, q).map(Err);
    }
    // Кто меняет (решение 59): обмен должен сделать сам этот пул — иначе пул исключён, проверка «не проверено».
    // Через исполнитель (tradable = false) — событие самого пула по адресу шага (check/probe_exec.rs::emitter).
    probe_exec::emitter(ctx, e, &first[x].logs).await?;
    let (Some(buy_expected), Some(bought)) = (
        way.expected(ctx, e, &first[q].return_data, usdt, usdt_amount)?,
        probe_buy::bought(&first[h.before], &first[h.after]),
    ) else {
        return Ok(Err("ответ квотера или баланса не по ABI".into()));
    };
    if let Some(min) = min
        && let Some(err) = probe_tiny::buy(ctx, e, buy_expected, min)
    {
        return Err(err);
    }
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
        vista.as_deref(),
    )
    .await?;
    // Решение 252: продажа — СРАЗУ после покупки, пробный перевод четверти — ПОСЛЕ продажи (порядок — pc::sell_tail).
    // Настоящая продажа (Universal Router, исполнитель, роутер форка) идёт без перевода монеты перед ней в том же блоке;
    // у монеты с отражением (reflection, форк SafeMoon) перевод перед продажей поднял бы balanceOf пары выше резерва, и
    // пара V2 (вход = balanceOf − резерв) засчитала бы продаже лишний вход — налог продажи пробы занижен.
    let (sq_to, sq_data) = way.request(ctx, e, coin, rest)?;
    let base = list.len();
    let mut t = pc::sell_tail(
        &mut list,
        (wallet, coin, usdt, empty),
        quarter,
        &sell_aps,
        (sq_to, sq_data),
        (sell_router, sell_data),
    );
    if let Some(v) = &vista {
        values.push((t.quote + 2, v.value(coin, rest)));
        if let Some(b) = v.balance(wallet, usdt) {
            list[t.quote + 1] = b.clone();
            list[t.quote + 3] = b;
        }
        if let Some(w) = v.wrap_for_transfer(wallet, coin) {
            values.push((t.transfer, quarter));
            list.insert(t.transfer, w);
            t.transfer += 1;
        }
    }
    let all = simulate::run_anchored(ctx, &list, &values, &over, at, hash).await?;
    if let Some(r) = all.iter().find(|r| pc::late(r)) {
        let m = format!(
            "TransactionDeadlinePassed: срок транзакции вышел к симулированному блоку (время головы + 1 с, check/sim_base.rs) — проверьте dex.deadline_s и свежесть узла: {}",
            r.why()
        );
        return Err(ctx.err(ErrorCode::NoRoute, m));
    }
    // Откат вызовов покупки (повтор первой симуляции) или пробного перевода — вердикт, как до решения 252.
    let transfer = t.transfer..t.transfer + 2;
    let mut bad = all
        .iter()
        .enumerate()
        .filter(|(i, _)| *i < base || transfer.contains(i));
    if let Some((i, r)) = bad.find(|(_, r)| !r.ok()) {
        return Ok(Err(format!(
            "шаг {i} пробы (перевод монеты) откатился: {}",
            r.why()
        )));
    }
    let Some(xb) = pc::u256(&all[t.transfer + 1]) else {
        return Ok(Err("ответ баланса не по ABI".into()));
    };
    let sell_appr = &all[base..t.quote];
    let sell_swap = &all[t.quote + 2];
    let sold = sell_swap.ok() && sell_appr.iter().all(|c| c.ok());
    if sold {
        probe_exec::emitter(ctx, e, &sell_swap.logs).await?;
    }
    let gas = |r: &[simulate::CallResult]| r.iter().map(|c| c.gas_used.to::<u64>()).sum::<u64>();
    Ok(Ok(Measured {
        buy_expected,
        buy_got: bought,
        sell: {
            let got = pc::sell_result(ctx, e, way, &all[t.quote..t.quote + 4], (sold, rest));
            match min {
                Some(min) => probe_tiny::sell(ctx, e, way, &all[t.quote], rest, min, got),
                None => got,
            }
        },
        transfer_sent: quarter,
        transfer_got: xb,
        gas_buy: all[x].gas_used.to(),
        gas_approve_buy: gas(&all[approvals_start..approvals_start + n_appr]),
        gas_transfer: all[t.transfer].gas_used.to(),
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
    vista: Option<&super::vista_probe::Probe>,
) -> GatewayResult<(Vec<Approval>, Address, Bytes)> {
    if let Some(v) = vista {
        return v.leg(ctx, e, &order, wallet, now);
    }
    // Решение 122 F/X: у вида без своего пути исполнения (family.tradable = false: DODO, THENA Fusion, Solidly,
    // Slipstream, форки V3, StableSwap, Maverick) пробу не собрать ни через Universal Router (чужой пул он не найдёт), ни
    // через classic V2 Router форка (у V3-форка его нет). Путь сделки для них — шаг контракта-исполнителя: тем же
    // вызовом execute идёт и проба (check/probe_exec.rs); нет шага / не задан адрес — точная причина оттуда.
    if super::super::quote::ekubo_v2_pricing::enabled(e){return super::super::exec::ekubo_v2_direct::probe(ctx,e,&order,now);}
    if super::super::exec::vault_direct::enabled(e) && super::super::exec::executor::address(ctx)?.is_none() {
        return Box::pin(super::super::exec::vault_direct::probe(ctx,e,order,wallet,now)).await;
    }
    if super::super::quote::vista_pricing::is_vista(e) {
        if e.family.executor_kind.is_some() && super::super::exec::executor::address(ctx)?.is_some()
        {
            return Box::pin(probe_exec::part(ctx, e, order, wallet, now)).await;
        }
        return Box::pin(super::super::exec::vista_direct::probe(
            ctx, e, order, wallet, now,
        ))
        .await;
    }
    if super::super::quote::oasis_pricing::is_oasis(e) {
        if e.family.executor_kind.is_some() && super::super::exec::executor::address(ctx)?.is_some()
        {
            let head = super::super::quote::white_state::head(ctx, "latest").await?;
            Box::pin(super::super::quote::oasis_pricing::state(
                ctx, e, wallet, head, true,
            ))
            .await?;
            return Box::pin(probe_exec::part(ctx, e, order, wallet, now)).await;
        }
        return Box::pin(super::super::exec::oasis_direct::probe(
            ctx, e, order, wallet, now,
        ))
        .await;
    }
    if super::super::quote::frax_pricing::is_frax(e)
        && ctx
            .dex_text("dex.fraxswap_router_hash", e.family.id)
            .is_ok()
    {
        let direct = Box::pin(super::super::exec::frax_direct::probe(
            ctx, e, &order, wallet, now,
        ))
        .await;
        if direct.is_ok() || e.family.executor_kind.is_none() {
            return direct;
        }
    }
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
