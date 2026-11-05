//! Файл: venues/bsc_dex/exec/executor/trade.rs
//! Что делает: сделка по графовому маршруту ОДНОЙ транзакцией через свой исполнитель ArbExecutor (решение 122, часть X)
//! — вместо бандла (решение 84), когда задан exec.executor_address. Доли — по живому состоянию пулов (как бандл и
//! сухая сделка: exec/dry_paths.rs, exec/live.rs — проба крючка и допуск, решение 32), средняя цена против худшей,
//! минимум каждой доли — из предторговой верификации (решение 83), шаги исполнителя (call.rs), очередь кошелька
//! (решение 45), разрешение ERC-20 исполнителю (один раз на токен; отправленное разрешение ждёт блока — тогда всё
//! собирается заново на живом состоянии со свежей верификацией, новое разрешение после этого — отказ, решение 121 B1;
//! до разрешения код и владелец исполнителя сверяются с узлом — onchain.rs),
//! проба execute перед отправкой со сверкой req.worst_price (sim.rs), одна транзакция существующими каналами
//! (exec/txs.rs), факт по событиям Transfer (fact.rs). Повторов нет (стартовое правило Тиграна).
//! Отметка «идёт обмен» (wallet/inflows.rs) не нужна: минимум проверяет сам исполнитель по приросту баланса кошелька
//! внутри транзакции, а не по базе, прочитанной до подписи (как BALANCE_CHECK_ERC20 Universal Router).
//! Нативная монета (решение 62) на краях маршрута через исполнитель не исполняется: вход и выход — только ERC-20.
//! Внутри шага STABLE_U256_BNB (пулы с меткой 0xEeee…) BNB есть — контракт разворачивает/заворачивает WBNB сам.
//! Кто вызывает: exec/trade.rs (графовый маршрут формы «бандл» при заданном exec.executor_address).
//! Решение 122 F: хопы через пулы Maverick V2 (порт чужой математики) сверяются с квотером в prepare (exec/precheck_quoter.rs).
//! От чего зависит: exec/executor/{call, sim, fact, onchain}.rs, exec/{chain, dry_paths, native, order, precheck_quoter, swap_send, txs,
//! verify_gate}.rs, quote/{via, direct}.rs, wallet/{approve, nonce}.rs.
//! Настройки: exec.executor_address [bsc_dex]; через вызываемые — dex.deadline_s, dex.approve_unlimited,
//! exec.dex_slippage_pct, verify.*, send.*, channel.*.
//! Уведомление: dex.rebuilt_after_approve (как у прямого и умного пути).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{DexRoute, Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::quote::via::Via;
use super::super::super::quote::{Quoting, direct};
use super::super::super::wallet::approve_executor;
use super::super::super::wallet::nonce::Nonces;
use super::super::chain::{self, Step};
use super::super::dry_paths::live_paths;
use super::super::native;
use super::super::order::{check_price, worst_positive};
use super::super::swap_send::{rebuilt_notice, send_approvals};
use super::super::trade::sides;
use super::super::txs::{self, Env};
use super::super::verify_gate::{self, Verified};
use super::call::{self, ExecCall};
use super::{fact, onchain, sim};

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
    if native::is_native(ctx, &req.asset) {
        let why = "нативная монета через исполнитель (решение 122) в v1 не исполняется";
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
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
    let (legs, mut note) = prepare(&v, &req, route, &verified.amount_out_min, decs).await?;
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    // Ревью 122, пункт 1: адрес вписан руками — до approve и сделки код и владелец сверяются с узлом.
    onchain::ensure(ctx, executor, wallet).await?;
    let (mut c, mut at) = build(q, &legs, executor).await?;
    let need = approve_executor::missing(ctx, c.token_in, wallet, executor, c.amount_in).await?;
    if send_approvals(env, &slot, &need).await? {
        // Решение 121 B1: разрешение ждало блока — доли, минимумы и вызов устарели: свежая верификация и сборка.
        let mins = verify_gate::gate(q, env, &req, route).await?.amount_out_min;
        let (legs, fresh) = prepare(&v, &req, route, &mins, decs).await?;
        note = fresh;
        (c, at) = build(q, &legs, executor).await?;
        if !approve_executor::missing(ctx, c.token_in, wallet, executor, c.amount_in)
            .await?
            .is_empty()
        {
            let why = "после approve свежий маршрут требует нового разрешения исполнителю — своп не отправлен";
            return Err(ctx.err(ErrorCode::QuoteMismatch, why));
        }
        rebuilt_notice(env, &req);
    }
    sim::check(ctx, &req, &c, wallet, at, decs).await?;
    let d = txs::send(env, &slot, executor, c.data(), U256::ZERO, true, None).await?;
    drop(slot);
    let mut r = fact::result(
        ctx,
        req,
        d,
        (wallet, executor),
        (c.token_in, c.token_out),
        decs,
    )?;
    // Решение 177: «продажа по квотеру» — в хронологию сделки (exec/result.rs by допишет её к исполнителю).
    if note.is_some() {
        r.executor = note;
    }
    Ok(r)
}

/// Доли по живому состоянию: пути, сверка средней цены с худшей, шаги exec/chain.rs и минимум каждой доли из
/// верификации. Решение 177: продажа выхода с расхождением хопа Maverick — доля по квотеру, минимум верификации — тем
/// же множителем. Принимает: расчёт путей, запрос, маршрут, минимумы долей, (decimals монеты, USDT).
/// Возвращает: шаги долей и строку «— продажа по квотеру» (None — по расчёту).
/// Ошибки: NoRoute, QuoteMismatch (цена, число долей), CoinNotChecked, BadResponse, узел.
pub async fn prepare(
    v: &Via<'_>,
    req: &TradeRequest,
    route: &DexRoute,
    mins: &[Decimal],
    decs: (u8, u8),
) -> GatewayResult<(Vec<Vec<Step>>, Option<String>)> {
    let ctx = &v.q.ctx;
    let mut paths = live_paths(v, req, route, false).await?;
    // Решение 122 F: хопы через пулы с портом чужой математики (Maverick V2) — сверка квотером на своём узле.
    let (ratios, note) = super::super::precheck_quoter::hops_exit(ctx, req, &mut paths).await?;
    let (coins, paid) = paths
        .iter()
        .fold((Decimal::ZERO, Decimal::ZERO), |(c, u), (p, _)| {
            (c + p.coins, u + p.usdt)
        });
    check_price(ctx, req, coins, paid)?;
    if mins.len() != paths.len() {
        let why = "верификация вернула число долей, не равное маршруту";
        return Err(ctx.err(ErrorCode::QuoteMismatch, why));
    }
    let out_dec = if req.side == Side::Buy {
        decs.0
    } else {
        decs.1
    };
    let mut legs = Vec::with_capacity(paths.len());
    for (((p, qty), min), r) in paths.iter().zip(mins).zip(&ratios) {
        let mut steps = chain::steps(ctx, req, p, *qty, decs)?;
        verify_gate::apply_leg_min(ctx, &mut steps, *min * *r, out_dec)?;
        legs.push(steps);
    }
    Ok((legs, note))
}

/// Вызов execute на текущем блоке. Возвращает: вызов и (номер, время) блока (поверх него — проба). Ошибки: как
/// call::build, узел.
async fn build(
    q: &Quoting,
    legs: &[Vec<Step>],
    executor: Address,
) -> GatewayResult<(ExecCall, (u64, u64))> {
    let (at, now) = q.heads.at(&q.ctx).await?;
    Ok((call::build(&q.ctx, legs, executor, now)?, (at, now)))
}
