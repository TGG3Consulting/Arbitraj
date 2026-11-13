//! Файл: core/sell_range_run.rs
//! Что делает: решение 268 — исполнение ветки «лимитный ордер продажи отклонён: цена вне диапазона биржи» (когда идти в
//! ветку и что делать — core/sell_range.rs): отметка сделки (Deal.price_range_tried), свежая котировка продажи
//! (trade_ops::deal_quote → exec_book.rs: память, если свежая, иначе REST), строка хронологии и журнал core.sell_range,
//! рыночный ордер биржи (trade_ops::trade_seen с признаком market → контракт trade_market_sell) или переход в режим
//! застрявшей монеты. Все будущие ветки собираются в коробки в отдельных не-async функциях: состояние ветки не занимает
//! место в задаче продажи sell.rs и в задачах, которые её вкладывают (stuck.rs, relay.rs) — глубина стека у продажи
//! без этой ветки не растёт (замер — docs/ROADMAP.md, «Техдолг»).
//! Кто вызывает: core/sell.rs (run — после wanted; stuck — переход в режим застрявшей монеты).
//! От чего зависит: sell_range.rs (decide, Act, Outcome), trade_ops.rs (deal_quote, trade_seen), sell.rs (profitable,
//! BUTTON), stuck.rs (handle), deal_gas.rs, ledger.rs, exec_book.rs, stuck_reason.rs, op_error.rs, deps.rs.
//! Настройки: нет (включатель sell.market_on_price_range читает sell_range.rs).

use std::future::Future;
use std::pin::Pin;

use rust_decimal::Decimal;

use crate::core::types::{Quote, Side, TradeResult};
use crate::gateway::errors::ErrorCode;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;
use super::sell_range::{Act, Outcome, decide};
use super::stuck_reason::StuckReason;

/// Что делает: ветка решения 268 (вызывать только при wanted): отметка сделки, свежая котировка, решение, строка
/// хронологии и журнал core.sell_range, рыночный ордер. Принимает: ядро, сделку (отметка, хронология и газ пишутся в
/// неё), ошибку лимитного ордера, причину продажи (sell::BUTTON — кнопка). Возвращает: Outcome. Ошибок нет.
///
/// Будущее сразу в коробке и собирается в отдельной функции (не async): его состояние (котировка, рыночный ордер) не
/// занимает место в кадре стека задачи продажи sell.rs — у продажи без этой ветки глубина стека не растёт.
#[inline(never)]
pub fn run<'a>(
    core: &'a Core,
    deal: &'a mut Deal,
    e: &'a OpError,
    why: &'a str,
) -> Pin<Box<dyn Future<Output = Outcome> + Send + 'a>> {
    Box::pin(run_inner(core, deal, e, why))
}

/// Тело ветки (см. run). Принимает и возвращает: как run. Ошибок нет.
async fn run_inner(core: &Core, deal: &mut Deal, e: &OpError, why: &str) -> Outcome {
    deal.price_range_tried = true;
    let button = why == super::sell::BUTTON;
    let head =
        format!("Биржа отклонила лимитный ордер продажи: цена вне разрешённого диапазона ({e})");
    let quote = quote_boxed(core, deal).await;
    let good = quote
        .as_ref()
        .ok()
        .map(|(v, _, _)| super::sell::profitable(deal, *v));
    let act = decide(e.code, e.sent, true, true, false, button, good);
    let (value, q, book) = match (act, quote) {
        (Act::Market, Ok(x)) => x,
        (Act::Stuck, Ok((v, _, _))) => {
            let text = format!(
                "{head}. По стакану продажа здесь невыгодна (оценка выручки {v}) — режим застрявшей монеты (решение 268)"
            );
            note(core, deal, text).await;
            return Outcome::Stuck(StuckReason::Unprofitable);
        }
        (Act::Stuck, Err(err)) => {
            let text = format!(
                "{head}. Котировку продажи получить нельзя: {err} — режим застрявшей монеты (решение 268)"
            );
            note(core, deal, text).await;
            return Outcome::Stuck(StuckReason::NoQuote(err.message));
        }
        (_, Err(err)) => {
            let m = format!("{e} — рыночная продажа невозможна: нет котировки ({err})");
            note(
                core,
                deal,
                format!("{head}. Котировки нет: {err} (решение 268)"),
            )
            .await;
            return Outcome::Failed(OpError {
                message: m,
                ..e.clone()
            });
        }
        (_, Ok(_)) => return Outcome::Failed(e.clone()),
    };
    let src = match &book {
        Some(b) => {
            let age = b
                .snap
                .book()
                .map(|x| (chrono::Utc::now() - x.received_at).num_milliseconds());
            let age = age.map_or("неизвестен".to_string(), |a| format!("{a} мс"));
            let rest = if b.rest { ", снимок REST" } else { "" };
            format!("По стакану (возраст {age}{rest})")
        }
        None => "По котировке".to_string(),
    };
    let verdict = if button {
        "кнопка человека — без проверки выгодности"
    } else {
        "продажа выгодна"
    };
    let text = format!(
        "{head}. {src} {verdict}: оценка выручки {value} USDT (средняя {}) — продаём рыночным ордером (решение 268)",
        q.avg_price
    );
    note(core, deal, text).await;
    let v = deal.at.clone();
    let (mut seen, mut gas) = (None, Vec::new());
    let res = market_order(core, deal, &q, book, &mut seen, &mut gas).await;
    super::deal_gas::charge(core, deal, &v, &mut gas).await;
    match res {
        Ok(r) => {
            deal.note(format!(
                "Рыночный ордер продажи исполнен: {} по средней {} (решение 268)",
                r.filled_qty, r.avg_price
            ));
            Outcome::Sold(Box::new(r))
        }
        // Площадка рыночную продажу не умеет — как раньше: алерт по исходному отказу, с причиной.
        Err(m) if m.code == Some(ErrorCode::NotApplicable) => Outcome::Failed(OpError {
            message: format!("{e} — рыночная продажа невозможна: {m}"),
            ..e.clone()
        }),
        Err(m) => Outcome::Failed(OpError {
            message: format!("рыночный ордер продажи после отказа «цена вне диапазона»: {m}"),
            ..m
        }),
    }
}

/// Режим застрявшей монеты после ветки (stuck::handle, как sell::sell_or_stuck) — будущее в коробке, собранное вне
/// кадра задачи продажи (глубина стека sell.rs не растёт). Принимает: ядро, сделку, причину. Ошибок нет.
#[inline(never)]
pub fn stuck<'a>(
    core: &'a Core,
    deal: Deal,
    why: StuckReason,
) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
    Box::pin(super::stuck::handle(core, deal, why))
}

/// Свежая котировка продажи сделки (trade_ops::deal_quote) — будущее в коробке, собранное вне кадра ветки (глубина
/// стека). Принимает: ядро, сделку. Возвращает и ошибки: как deal_quote.
#[inline(never)]
fn quote_boxed<'a>(
    core: &'a Core,
    deal: &'a Deal,
) -> Pin<Box<dyn Future<Output = Quoted> + Send + 'a>> {
    Box::pin(super::trade_ops::deal_quote(core, deal))
}

/// Итог котировки продажи: (выручка чистыми, котировка, стакан котировки биржи).
type Quoted = Result<(Decimal, Quote, Option<super::exec_book::Book>), OpError>;

/// Рыночный ордер продажи всего количества сделки (trade_ops::trade_seen с признаком market) — будущее в коробке,
/// собранное вне кадра ветки (глубина стека). Принимает: ядро, сделку, котировку, её стакан (худшая цена запроса — по
/// нему), место для худшей цены и газа вне итога. Возвращает и ошибки: как trade_seen.
#[inline(never)]
fn market_order<'a>(
    core: &'a Core,
    deal: &'a Deal,
    q: &'a Quote,
    book: Option<super::exec_book::Book>,
    seen: &'a mut Option<super::trade_ops::Worst>,
    gas: &'a mut Vec<super::deal_gas::Wasted>,
) -> Pin<Box<dyn Future<Output = Result<TradeResult, OpError>> + Send + 'a>> {
    Box::pin(super::trade_ops::trade_seen(
        core,
        &deal.id,
        &deal.at,
        &deal.direction.asset,
        Side::Sell,
        deal.qty,
        q.route.clone(),
        q.avg_price,
        None,
        None,
        book,
        seen,
        Some(gas),
        None,
        true,
    ))
}

/// Строка в хронологию сделки (с сохранением) и в журнал core.sell_range. Ошибок нет.
async fn note(core: &Core, deal: &mut Deal, text: String) {
    deal.note(text.clone());
    super::ledger::save(core, deal).await;
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.sell_range",
            Some(&deal.at.0),
            format!("{}: {text}", deal.id),
            None,
        )
        .await;
}
