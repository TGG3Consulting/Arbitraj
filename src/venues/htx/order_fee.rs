//! Файл: venues/htx/order_fee.rs
//! Что делает: факт торговой комиссии ордера HTX по его сделкам (matchresults): сколько и в какой монете — в монете
//! сделки (fee-currency) или списано скидкой HT / баллами HB (fee-deduct-currency, урок 23).
//! Решение 264: офдока не говорит, что сделки ордера видны в matchresults сразу после окончательного state, — пустой
//! или неполный список у исполненного ордера не «комиссия 0» (filled-fees детали ордера — «Accurate fees refer to
//! matchresults endpoint», монеты комиссии там нет): matchresults читается повторно, пока Σ filled-amount
//! сделок не сравняется с filled-amount ордера, не дольше venue.order_final_wait_ms; не сравнялась — ошибка (у
//! вызывающего — «итог неизвестен»).
//! Кто вызывает: order_state.rs (result), tests/htx_real_data.rs, tests/order_reply_264.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms [htx] — через вызывающего (order_state::times).
//!
//! Официально — Get the Match Result of an Order GET /v1/order/orders/{order-id}/matchresults («50 times/2s»)
//! https://huobiapi.github.io/docs/spot/v1/en/#get-the-match-result-of-an-order: filled-fees «positive for a fee;
//! negative for a maker rebate» (бот ставит только IOC/FOK — тейкер), fee-currency — монета комиссии,
//! fee-deduct-currency — пусто: комиссия в исходной монете, «ht» или «hbpoint» — списано ими; filled-points — сколько
//! списано HT / баллами; FAQ: filled-fees и filled-points не бывают заполнены оба; fee-deduct-state «ongoing» / «done».
//! fee-currency: «transaction fee of buy order is based on base currency, transaction fee of sell order is based on
//! quote currency» (покупка — в монете, продажа — в котируемой; модель котировки book.rs та же).
//! Решение 232 (образец 219): сделки ордера в разных монетах (часть — HT, часть — монета сделки) — несколько частей
//! комиссии списком, а не ошибка.
//! Баллы HB (fee-deduct-currency «hbpoint», «the transaction fee is deducted by HB point»; filled-points «deduction amount
//! (unit: in ht or hbpoint)») — монета HBPOINT, рынка у неё нет, поэтому часть несёт стоимость в USDT (Fee.usd) по курсу
//! справки HTX «Point Card Usage Instructions» (https://www.huobi.br.com/support/360000182831 — справочный центр HTX,
//! страница ссылается на HTX APP; адрес htx.com из среды разработки недоступен — прокси 403): «The fee will be deducted by the
//! points you have (1 pts = 1 USDT fee)», «If your fee is paid in other coins, the fee will be estimated into USDT
//! according to the exchange rate. Your Point Card will pay USDT for it»; то же — объявление Huobi Global 17.06.2019
//! (https://huobiglobal.zendesk.com/hc/en-us/articles/360000302581): «1 Point (Point Card):1USDT». Баллы HB — счёт
//! «point: Point card account» (Glossary, Get Point Balance GET /v2/point/account). API цены баллов в документации нет.

use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::Fee;

use super::ctx::{Ctx, ticker};
use super::{num, rest};

/// fee-deduct-currency, когда комиссия списана баллами HB (Get the Match Result of an Order: «if showing value as
/// "hbpoint", the transaction fee is deducted by HB point»). Значение задано документацией API.
const HBPOINT: &str = "hbpoint";

/// USDT за один балл: «1 pts = 1 USDT fee» (справка HTX «Point Card Usage Instructions»), «1 Point (Point Card):1USDT»
/// (объявление Huobi Global 17.06.2019) — курс задан официальной документацией площадки, не торговый параметр.
const POINT_USDT: Decimal = Decimal::ONE;

/// Что делает: комиссия ордера по его сделкам (запрос matchresults), части по монетам; пока Σ filled-amount сделок
/// не равна исполненному количеству ордера (сделки ещё не видны) — повтор через poll, не дольше wait (решение 264).
/// Принимает: окружение, order-id, получаемую монету сделки (если у сделки нет fee-currency), исполненное количество
/// ордера (filled-amount детали), сроки poll и wait.
/// Возвращает: список частей комиссии (core/types/trade.rs Fee). Ошибка — текст причины (запрос не прошёл, ответ не
/// тот, сделки за срок не сошлись с исполненным).
pub async fn fact(
    ctx: &Ctx,
    order_id: &str,
    received: &str,
    filled: Decimal,
    poll: Duration,
    wait: Duration,
) -> Result<Vec<Fee>, String> {
    let path = format!("/v1/order/orders/{order_id}/matchresults");
    let started = Instant::now();
    loop {
        let why = match rest::get(ctx, &path, vec![]).await {
            Ok(data) => {
                let got = traded(&data)?;
                if got == filled {
                    return parse(&data, received);
                }
                format!(
                    "сделки ордера (matchresults): исполнено по сделкам {got}, у ордера {filled}"
                )
            }
            Err(e) => format!("сделки ордера (matchresults): {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(why);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Что делает: сколько исполнено по сделкам matchresults — Σ filled-amount («The amount which has been filled»,
/// Get the Match Result of an Order). Принимает: data (массив сделок; null — сделок нет).
/// Возвращает: сумму (нет сделок — 0). Ошибки: текст — не массив, filled-amount нет или не число.
pub fn traded(data: &Value) -> Result<Decimal, String> {
    let list = match data {
        Value::Null => return Ok(Decimal::ZERO),
        Value::Array(a) => a,
        other => return Err(format!("matchresults: data не массив: {other}")),
    };
    list.iter().try_fold(Decimal::ZERO, |acc, m| {
        num::dec(m, "filled-amount")
            .map(|v| acc + v)
            .map_err(|_| format!("matchresults: filled-amount нет или не число: {m}"))
    })
}

/// Что делает: разбор data matchresults в части комиссии по монетам.
/// Принимает: data (массив сделок), получаемую монету (если у сделки нет fee-currency). Возвращает: список частей
/// (сделок нет — пустой; баллы HB — часть HBPOINT со стоимостью в USDT, Fee.usd). Ошибки: текст — не массив, поле не число.
pub fn parse(data: &Value, received: &str) -> Result<Vec<Fee>, String> {
    let list = match data {
        Value::Null => return Ok(Vec::new()),
        Value::Array(a) => a,
        other => return Err(format!("matchresults: data не массив: {other}")),
    };
    let mut fees = Vec::new();
    for m in list {
        let dec = |f: &str| {
            num::dec_opt(m, f)
                .map(|v| v.unwrap_or(Decimal::ZERO))
                .map_err(|_| format!("matchresults: {f} не число: {m}"))
        };
        let (coin, amount) = match num::text_opt(m, "fee-deduct-currency") {
            // Баллы HB: рынка нет — стоимость в USDT по курсу документации (шапка файла).
            Some(c) if c.eq_ignore_ascii_case(HBPOINT) => {
                let pts = dec("filled-points")?.abs();
                Fee::add_usd(&mut fees, &ticker(&c), pts, pts * POINT_USDT);
                continue;
            }
            Some(c) => (ticker(&c), dec("filled-points")?),
            None => {
                let c = num::text_opt(m, "fee-currency").unwrap_or_else(|| received.to_string());
                (ticker(&c), dec("filled-fees")?)
            }
        };
        Fee::add(&mut fees, &coin, amount.abs());
    }
    Ok(fees)
}
