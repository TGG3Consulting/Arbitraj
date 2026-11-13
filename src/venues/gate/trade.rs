//! Файл: venues/gate/trade.rs
//! Что делает: покупка или продажа на Gate лимитным ордером с худшей допустимой ценой и временем жизни ioc или fok
//! (venue.order_time_in_force, решение 135 — Тигран 04.11; что не исполнилось сразу — снимает биржа) и проверка факта (количество, средняя цена, комиссия — order_state.rs).
//! Правила пары, округление и пределы — order_rules.rs: та же функция у проверки ордера без исполнения (решение 134).
//! Кто вызывает: mod.rs (trade из контракта).
//! Решение 268: тот же путь постановки и итога — у рыночной продажи (market_sell.rs передаёт готовый ордер, prepared).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, symbols.rs, num.rs, ctx.rs.
//! Настройки: venue.order_time_in_force [gate] (ioc или fok); ожидание итога и поиск — order_state.rs (проверяются до
//! отправки).
//!
//! Официальная документация: POST /spot/orders https://www.gate.com/docs/developers/apiv4/en/ (SDK: docs/SpotApi.md
//! #create_order) — ответ 201 «Order created. The execution result depends on the time-in-force policy», тело — ордер
//! (id, status, filled_amount, filled_total, fee, fee_currency, finish_as; action_mode по умолчанию FULL — «Full mode»).
//! Лимит — «10r/s (UID+Market)» на постановку и изменение; правило доли исполнения (fill-ratio) — не больше 10
//! запросов за 10 с, если доля исполнения за 24 ч мала («Frequency limit rule»): бот ставит ордера только ioc/fok по худшей
//! цене, отказ лимита — RateLimited (правила шлюза).

use reqwest::Method;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ Gate), OutcomeUnknown —
/// ордер мог быть принят, итог не получен (решение 64).
/// Решение 268: `prepared` — уже подготовленный ордер (рыночная продажа, market_sell.rs): постановка и итог — тем же
/// кодом; None — обычный лимитный ордер по правилам order_rules::ready.
pub async fn trade(
    ctx: &Ctx,
    symbols: &Symbols,
    req: TradeRequest,
    prepared: Option<order_rules::Prepared>,
) -> GatewayResult<TradeResult> {
    let p = match prepared {
        Some(p) => p,
        None => order_rules::ready(ctx, symbols, &req).await?,
    };
    let started_s = chrono::Utc::now().timestamp();
    let placed = rest::signed(
        ctx,
        Method::POST,
        "/spot/orders",
        vec![],
        Some(p.body.clone()),
        Op::Trade,
    )
    .await;
    // HTTP успешен, а id нет — итог неизвестен (повтор дал бы второй ордер).
    let placed = placed.and_then(|r| match num::text_opt(&r, "id") {
        Some(id) => Ok((id, r)),
        None => {
            let m = format!(
                "{}: ордер принят без id в ответе — итог неизвестен",
                p.symbol
            );
            Err(ctx
                .tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, m))
                .with_raw(r.to_string()))
        }
    });
    // Решение 231: «чем найти» — к любому «итог неизвестен» ниже (ядро выяснит исход само, outcome_resolve.rs).
    let pending = PendingRef {
        sent_at_s: Some(started_s),
        ..PendingRef::order(&p.symbol, &p.text, p.qty)
    };
    let (order_id, first) = match placed {
        Ok((id, r)) => (id, Some(r)),
        // Итог неизвестен — поиск по своему text; не найден — отмена по нему и поиск в истории (как MEXC, А-🟠4).
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.symbol, &p.text, started_s).await {
                Some(id) => (id, None),
                None => return Err(e.pending(pending)),
            }
        }
        Err(e) => return Err(e),
    };
    let by_id = pending.with_ref(&order_id);
    let state = match first.filter(order_state::is_final) {
        Some(s) => s,
        None => order_state::settle(ctx, &p.symbol, &order_id)
            .await
            .map_err(|e| e.pending(by_id.clone()))?,
    };
    order_state::result(ctx, req, &state, order_id, p.qty).map_err(|e| e.pending(by_id))
}
