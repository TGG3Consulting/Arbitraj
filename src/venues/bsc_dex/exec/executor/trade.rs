//! Файл: venues/bsc_dex/exec/executor/trade.rs
//! Что делает: сделка по графовому маршруту ОДНОЙ транзакцией через свой исполнитель ArbExecutor (решение 122, часть X)
//! — вместо бандла (решение 84), когда задан exec.executor_address. Доли — по живому состоянию пулов (как бандл и
//! сухая сделка: exec/dry_paths.rs, exec/live.rs — проба крючка и допуск, решение 32), средняя цена против худшей,
//! минимум каждой доли — решение 228 (exec/leg_min.rs: E_k × max(1 − допуск, B / E), минимум предторговой
//! верификации, решение 83, — нижней границей; minOut = сумма = max(E × (1 − допуск), B)), шаги исполнителя (call.rs), очередь кошелька
//! (решение 45), разрешение ERC-20 исполнителю (один раз на токен; отправленное разрешение ждёт блока — тогда всё
//! собирается заново на живом состоянии со свежей верификацией, новое разрешение после этого — отказ, решение 121 B1;
//! до разрешения код и владелец исполнителя сверяются с узлом — onchain.rs),
//! проба execute перед отправкой со сверкой req.worst_price (sim.rs; откат — с minOut, ожидаемым и модельным налогом
//! каждой доли, решение 234 п. 4; решение 247 — откат по minOut у продажи выхода: пере-замер, remeasure.rs), одна
//! транзакция существующими каналами
//! (exec/txs.rs), факт по событиям Transfer (fact.rs). Повторов нет (стартовое правило Тиграна).
//! Отметка «идёт обмен» (wallet/inflows.rs) не нужна: минимум проверяет сам исполнитель по приросту баланса кошелька
//! внутри транзакции, а не по базе, прочитанной до подписи (как BALANCE_CHECK_ERC20 Universal Router).
//! Нативная монета (решение 62) на краях маршрута через исполнитель v1 не исполняется: вход и выход — только ERC-20.
//! Решение 272: исполнитель v2 (mod.rs::native_capable) — executeNative: продажа BNB с value (сверх резерва на газ,
//! exec/native.rs::reserve_ok), покупка — BNB нативным на кошелёк; факт — по событию NativeSettled (fact.rs).
//! Внутри шага STABLE_U256_BNB (пулы с меткой 0xEeee…) BNB есть — контракт разворачивает/заворачивает WBNB сам.
//! Решение 273: перед отправкой — пауза и предел цены газа фабрик пар Solidly заново (exec/solidly_gate.rs fresh_cap),
//! предел — по цене самой транзакции (exec/txs.rs send_capped).
//! Задача t114: отказ нативной монеты — точная причина native_block (не сверен / v1 / обёртка не та); после сверки кода
//! (onchain::ensure записал версию у кошелька) обратный вызов пулов маршрута ещё раз проверяется по сверенной версии
//! (pools/exec_callback.rs: версия у кошелька могла быть записана старой сборкой бота). Задача t122 (п. 1): версия —
//! прямо из результата onchain::ensure (callback_at.rs missing_at), а не из записи у кошелька.
//! Кто вызывает: exec/trade.rs (графовый маршрут формы «бандл» при заданном exec.executor_address).
//! Решение 122 F: хопы через пулы Maverick V2 (порт чужой математики) сверяются с квотером в prepare (exec/precheck_quoter.rs).
//! От чего зависит: exec/executor/{call, callback_at, sim, fact, onchain}.rs, exec/{swap_ctx,chain, dry_paths, leg_min, native, order, precheck_quoter, swap_send, txs,
//! verify_gate}.rs, quote/{via, direct}.rs, wallet/{approve, nonce}.rs.
//! Настройки: exec.executor_address [bsc_dex]; через вызываемые — dex.deadline_s, dex.approve_unlimited,
//! exec.dex_slippage_pct, verify.*, send.*, channel.*.
//! Уведомление: dex.rebuilt_after_approve (как у прямого и умного пути).

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{DexRoute, Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::quote::path::Path;
use super::super::super::quote::via::Via;
use super::super::super::quote::{Quoting, direct, pool_tax};
use super::super::super::wallet::nonce::Nonces;
use super::super::chain::{self, Step};
use super::super::dry_paths::live_paths;
use super::super::leg_min;
use super::super::native;
use super::super::order::{check_price, worst_positive};
use super::super::swap_ctx::SwapCtx;
use super::super::swap_send::{rebuilt_notice, send_approvals, with_approve_gas};
use super::super::trade::sides;
use super::super::txs::{self, Env};
use super::super::verify_gate::{self, Verified};
use super::super::{quoter_refetch, quoter_refetch_check};
use super::call::{self, ExecCall};
use super::prep_refetch::{self, Prepared};
use super::{fact, onchain, remeasure, sim};

/// Сделка через исполнитель. Принимает: котировку, окружение отправки, очередь кошелька, запрос, маршрут, итог
/// верификации (минимумы долей), адрес исполнителя. Ошибки: NoRoute (нативная, доля не годится, пул без шага
/// исполнителя, проба откатилась), QuoteMismatch (цена хуже допустимой, новое разрешение после approve), StaleData,
/// настройки, каналы, узел, SwapReverted / TxNotIncluded / OutcomeUnknown (с фактом транзакции).
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: TradeRequest,
    route: &DexRoute,
    verified: Verified,
    executor: Address,
) -> GatewayResult<TradeResult> {
    let ctx = &q.ctx;
    worst_positive(ctx, &req)?;
    let is_native = native::is_native(ctx, &req.asset);
    // Задача t114 (3а): причина — из native_block (не сверен / v1 / обёртка не та / настройка не задана).
    let cash_target=super::super::super::quote::vista_pricing::native_cash_target(route);
    if cash_target.is_some_and(|t|t!=executor) {
        return Err(ctx.err(ErrorCode::NoRoute,"Изменился выбранный исполнитель расчёта в ETH"));
    }
    if (is_native || cash_target.is_some()) && let Some(why) = super::native_block(ctx) {
        return Err(ctx.err(ErrorCode::NoRoute, super::native_refusal(&why)));
    }
    // Решение 272: (обёртка, продажа ли) для executeNative; обёртка — dex.native_wrapped (is_native её уже прочитал).
    let nat = match native::wrapped(ctx) {
        Some((_, w)) if is_native => Some((w, req.side == Side::Sell)),
        Some((_, w)) if cash_target.is_some() => Some((w, req.side == Side::Buy)),
        _ => None,
    };
    let (coin, usdt, decs) = sides(q, &req).await?;
    let taxes = direct::taxes(q, &req.asset)?;
    let v = Via {
        q,
        coin,
        usdt,
        side: req.side,
        taxes: &taxes,
        coin_dec: decs.0,
        usdt_dec: decs.1,
    };
    let (legs, mut note, exp, mut favour) =
        prep_refetch::prepare(&v, &req, route, &verified.amount_out_min, decs).await?;
    if !super::super::super::pools::nami::path_ok(legs.iter().flatten().map(|s| &s.entry)) {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Сохранённый маршрут Nami содержит другие шаги",
        ));
    }
    // Решение 234 (п. 4): minOut, ожидаемое и модельный налог каждой доли — в текст отката пробы (хронология сделки).
    let mut diag = sim::legs_text(&legs, &exp, &req, decs);
    let mut model = exp;
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    // Ревью 122, пункт 1: адрес вписан руками — до approve и сделки код и владелец сверяются с узлом.
    let version = onchain::ensure(ctx, executor, wallet).await?;
    // Ревью 272 (Б5): v1 — уведомление один раз на кошелёк и адрес (v1_notice.rs).
    super::v1_notice::once(ctx, &env.stats, version).await;
    // Задача t114: обратный вызов пулов маршрута — по сверенной версии; t122 (п. 1): версия — из результата ensure.
    if let Some(why) = legs
        .iter()
        .flatten()
        .find_map(|s| super::callback_at::missing_at(ctx, s.entry.family, version))
    {
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    if nat.is_some() && !version.native() {
        let why = format!(
            "исполнитель {executor:#x}: код в сети — версия {}, executeNative нет — нативный BNB не исполняется",
            version.label()
        );
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    // Ревью 272 (А1): v2, но обёртка в его конструкторе ≠ dex.native_wrapped — нативный BNB не исполняется.
    if let (Some(_), Some(why)) = (nat, super::native_block(ctx)) {
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    // Для Frax адрес шага есть в самой инструкции V5. Не смешиваем её со сменившейся настройкой адаптера.
    let frax_generation = legs
        .iter()
        .flatten()
        .any(|s| {
            matches!(s.entry.family.variant,crate::venues::bsc_dex::pools::family_variant::Variant::FraxTwamm | crate::venues::bsc_dex::pools::family_variant::Variant::Oasis | crate::venues::bsc_dex::pools::family_variant::Variant::Surge)
        })
        .then(|| ctx.settings.generation());
    let vista_generation = legs.iter().flatten().any(|s|super::super::vista_gate::needs(&s.entry))
        .then(|| ctx.settings.generation());
    let (mut c, mut at) = build(q, &legs, executor, nat).await?;
    let need = super::approvals(ctx, &c, wallet, executor).await?;
    if frax_generation.is_some() && !need.is_empty() {
        let used: Vec<_> = legs.iter().flatten().map(|s| &s.entry).collect();
        super::super::solidly_gate::fresh_cap(q, &used).await?;
        crate::venues::bsc_dex::pools::fraxswap::unchanged(ctx, frax_generation)?;
    }
    if let Some(generation)=vista_generation && !need.is_empty() {
        Box::pin(super::super::vista_gate::check(ctx,legs.iter().flatten().map(|s|(&s.entry,s.order.token_in)).collect(),generation)).await?;
    }
    let approved = send_approvals(env, &slot, &need).await?;
    // Решение 236, дополнение (техлид 09.11): газ разрешений — в итог обмена и в любую ошибку после них (swap_send.rs).
    let out: GatewayResult<TradeResult> = async {
    if approved.is_some() {
        // Решение 121 B1: разрешение ждало блока — доли, минимумы и вызов устарели: свежая верификация и сборка.
        let mins = verify_gate::gate(q, env, &req, route).await?.amount_out_min;
        let (legs, fresh, exp, fav) = prep_refetch::prepare(&v, &req, route, &mins, decs).await?;
        note = fresh;
        favour = fav;
        diag = sim::legs_text(&legs, &exp, &req, decs);
        model = exp;
        (c, at) = build(q, &legs, executor, nat).await?;
        if !super::approvals(ctx, &c, wallet, executor).await?.is_empty()
        {
            let why = "после approve свежий маршрут требует нового разрешения исполнителю — своп не отправлен";
            return Err(ctx.err(ErrorCode::QuoteMismatch, why));
        }
        rebuilt_notice(env, &req, at.0);
    }
    // Решение 261: «в нашу сторону» — в журнал один раз, по последней подготовке (ревью: после approve — второй раз).
    super::super::quoter_favour::log(ctx, &req, &favour).await;
    // Решение 247: откат пробы по minOut у продажи выхода — пере-замер реального выхода (remeasure.rs).
    let fix = remeasure::probe(ctx, &req, &mut c, wallet, at, decs, &diag, &model, env).await?;
    if let Some(line) = fix {
        note = Some(note.map_or(line.clone(), |n| format!("{n}; {line}")));
    }
    // Решение 231 (дополнение): контекст разбора (с адресом исполнителя — его возврат) — в «чем найти» (exec/swap_ctx.rs).
    let tokens = (c.token_in, c.token_out);
    let mut sc = SwapCtx::swap(wallet, tokens, decs, &[], Some(executor), note.clone());
    // Ревью 272 (Б4): вызов executeNative — признаком в контексте (позднее выяснение не гадает по событию).
    sc.executor_native = c.native.is_some();
    // Решение 272: продажа нативного BNB — value = вход, только сверх резерва на газ (как прямой путь, swap_send.rs).
    let value = c.value();
    if !value.is_zero() {
        native::reserve_ok(env, wallet, (executor, &c.data()), value).await?;
    }
    // Решение 273: пары Solidly с паузой/пределом газа фабрики — заново с фабрики; предел проверяет send_capped.
    let used: Vec<_> = legs.iter().flatten().map(|s| &s.entry).collect();
    let cap = super::super::solidly_gate::fresh_cap(q, &used).await?;
    crate::venues::bsc_dex::pools::fraxswap::unchanged(ctx,frax_generation)?;
    if let Some(generation)=vista_generation {
        Box::pin(super::super::vista_gate::check(ctx,legs.iter().flatten().map(|s|(&s.entry,s.order.token_in)).collect(),generation)).await?;
    }
    let d = txs::send_capped(env, &slot, (executor, c.data(), value), true, None, cap)
        .await
        .map_err(|e| sc.attach(e))?;
    drop(slot);
    let sc = SwapCtx {
        tip_bnb: d.tip_bnb,
        ..sc
    };
    let mut r =
        fact::result(ctx, req, d, (wallet, executor), tokens, decs, c.native.is_some())
            .map_err(|e| sc.attach(e))?;
    // Решение 177: «продажа по квотеру» — в хронологию сделки (exec/result.rs by допишет её к исполнителю).
    if note.is_some() {
        r.executor = note;
    }
    Ok(r)
    }
    .await;
    with_approve_gas(out, approved)
}

/// Доли по живому состоянию: пути, сверка средней цены с худшей, шаги exec/chain.rs и минимум каждой доли по решению
/// 228 (exec/leg_min.rs; минимум верификации — нижней границей). Решение 177: продажа выхода с расхождением хопа Maverick — доля по квотеру, минимум верификации — тем
/// же множителем. Принимает: расчёт путей, запрос, маршрут, минимумы долей, (decimals монеты, USDT).
/// Возвращает: шаги долей, строку «— продажа по квотеру» (None — по расчёту) и по каждой доле (ожидаемый выход,
/// модельный налог монеты на её шаге, %) — для текста отката пробы (решение 234, п. 4), строку «в нашу сторону»
/// (решение 261; журнал — вызывающий, один раз).
/// Ошибки: NoRoute, QuoteMismatch (цена, число долей), CoinNotChecked, BadResponse, узел.
/// Решение 382: обёртка с перечитыванием пулов при расхождении — exec/executor/prep_refetch.rs prepare.
pub(super) async fn prepare_once(
    v: &Via<'_>,
    req: &TradeRequest,
    route: &DexRoute,
    mins: &[Decimal],
    decs: (u8, u8),
) -> GatewayResult<Prepared> {
    let ctx = &v.q.ctx;
    let mut paths = live_paths(v, req, route, false).await?;
    // Решение 122 F: хопы через пулы с портом чужой математики (Maverick V2) — сверка квотером на своём узле.
    let (ratios, note, favour) =
        super::super::precheck_quoter::hops_exit(ctx, req, &mut paths, quoter_refetch::block(v.q))
            .await?;
    let (coins, paid) = paths
        .iter()
        .fold((Decimal::ZERO, Decimal::ZERO), |(c, u), (p, _)| {
            (c + p.coins, u + p.usdt)
        });
    check_price(ctx, req, coins, paid)?;
    // Решение 382, ревью Н1: по перечитанным пулам цена не хуже плана больше допуска сверки.
    if quoter_refetch::block(v.q).is_some() {
        quoter_refetch_check::plan_gap(ctx, req, coins, paid)?;
    }
    if mins.len() != paths.len() {
        let why = "верификация вернула число долей, не равное маршруту";
        return Err(ctx.err(ErrorCode::QuoteMismatch, why));
    }
    let mut legs = Vec::with_capacity(paths.len());
    for (p, qty) in &paths {
        legs.push(chain::steps(ctx, req, p, *qty, decs)?);
    }
    // Решение 228: минимум доли = E_k × max(1 − допуск, B / E); minOut execute = их сумма = max(E × (1 − допуск), B).
    // Минимум верификации (× множитель квотера, решение 177) — нижняя граница доли, не выше её ожидаемого.
    let ver: Vec<Decimal> = mins.iter().zip(&ratios).map(|(m, r)| *m * *r).collect();
    let shares = paths.iter().map(|(p, q)| (p, *q));
    leg_min::set_leg_mins(ctx, req, shares, &mut legs, Some(&ver), decs)?;
    let exp = paths
        .iter()
        .map(|(p, _)| (expected(p, req.side), coin_tax(v, p)))
        .collect();
    Ok((legs, note, exp, favour))
}

/// Вызов execute (решение 272: при nat — executeNative) на текущем блоке. Принимает: доли, исполнитель, (обёртка,
/// продажа ли) или None. Возвращает: вызов и (номер, время) блока (поверх него — проба). Ошибки: как call::build и
/// ExecCall::with_native, узел.
async fn build(
    q: &Quoting,
    legs: &[Vec<Step>],
    executor: Address,
    nat: Option<(Address, bool)>,
) -> GatewayResult<(ExecCall, (u64, u64))> {
    let (at, now) = q.heads.at(&q.ctx).await?;
    let c = call::build(&q.ctx, legs, executor, now)?;
    let c = match nat {
        Some((w, wrap_in)) => c.with_native(&q.ctx, w, wrap_in)?,
        None => c,
    };
    Ok((c, (at, now)))
}

/// Ожидаемый выход доли по живому пересчёту: продажа — USDT, покупка — монеты. Ошибок нет.
pub fn expected(p: &Path, side: Side) -> Decimal {
    match side {
        Side::Sell => p.usdt,
        Side::Buy => p.coins,
    }
}

/// Модельный налог монеты сделки на её шаге доли (продажа — первый шаг, покупка — последний), % — тот же, что в
/// расчёте (quote/pool_tax.rs step_pct). Нет шагов — 0. Ошибок нет.
pub fn coin_tax(v: &Via<'_>, p: &Path) -> Decimal {
    let h = match v.side {
        Side::Sell => p.hops.first(),
        Side::Buy => p.hops.last(),
    };
    h.map_or(Decimal::ZERO, |h| {
        pool_tax::step_pct(&h.entry, &h.usable, v.coin, v.side, v.taxes)
    })
}
