//! Файл: core/sell_range_run.rs
//! Что делает: решение 268 — исполнение ветки «лимитный ордер продажи отклонён: цена вне диапазона биржи» (когда идти в
//! ветку и что делать — core/sell_range.rs): отметки сделки по площадке (Deal.range: market, stuck), свежая котировка
//! (trade_ops::deal_quote → exec_book.rs: память, если свежая, иначе REST), строка хронологии, журнал core.sell_range и
//! уведомление deal.sell_market_range (журнал и уведомление — ПОСЛЕ рыночного ордера: до него только запись сделки с
//! отметкой, ревью 2),
//! рыночный ордер биржи (trade_ops::trade_seen с признаком market → контракт trade_market_sell) или переход в режим
//! застрявшей монеты. Все будущие ветки собираются в коробки в отдельных не-async функциях: состояние ветки не занимает
//! место в задаче продажи sell.rs и в задачах, которые её вкладывают (stuck.rs, relay.rs) — глубина стека у продажи
//! без этой ветки не растёт (замер — docs/ROADMAP.md, «Техдолг»).
//! Кто вызывает: core/sell.rs (run — после wanted).
//! От чего зависит: sell_range.rs (decide, Act, Outcome), trade_ops.rs (deal_quote, trade_seen), sell.rs (profitable,
//! BUTTON), stuck.rs (handle), deal_gas.rs, ledger.rs, exec_book.rs, stuck_reason.rs, op_error.rs, deps.rs.
//! Настройки: нет (включатель sell.market_on_price_range читает sell_range.rs).

use std::future::Future;
use std::pin::Pin;

use rust_decimal::Decimal;

use crate::core::types::{Quote, Side, TradeResult};
use crate::gateway::errors::ErrorCode;
use crate::notify::human::{self as h, Msg};
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;
use super::sell_range::{Act, Fresh, Marks, Outcome, Rule, decide, marks};
use super::stuck_reason::StuckReason;

/// Отклонённый лимитный ордер: его котировка и худшая цена (None — до расчёта цены не дошло). Нужны кнопке человека,
/// когда свежей котировки нет: рыночный ордер уходит по ним (цена — только оценка суммы у адаптера).
#[derive(Clone, Copy)]
pub struct Prev<'a> {
    pub q: &'a Quote,
    pub worst: Option<Decimal>,
}

/// Что делает: ветка решения 268 (вызывать только при wanted): свежая котировка, решение (sell_range::decide), отметка
/// сделки на площадке, строка хронологии и журнал core.sell_range, уведомление deal.sell_market_range и рыночный ордер.
/// Принимает: ядро, сделку (отметки, хронология и газ пишутся в неё), итог лимитного ордера `res` (Err — отказ биржи;
/// в него же кладётся итог ветки: факт рыночного ордера или ошибка, с которой sell.rs идёт дальше как с ошибкой
/// продажи), причину продажи (для текста), условие продажи, котировку и худшую цену отклонённого лимитного ордера.
/// Возвращает: true — сделка ушла в режим застрявшей монеты (он уже отработал здесь, продаже делать больше нечего);
/// false — итог в `res`. Ошибок нет.
///
/// Будущее сразу в коробке и собирается в отдельной функции (не async), итог пишется в уже существующее место `res`:
/// состояние ветки не занимает место в задаче продажи sell.rs и в задачах, которые её вкладывают, — у продажи без этой
/// ветки глубина стека не растёт.
#[inline(never)]
pub fn run<'a>(
    core: &'a Core,
    deal: &'a mut Deal,
    res: &'a mut Result<TradeResult, OpError>,
    why: &'a str,
    rule: Rule,
    q: &'a Quote,
    seen: &'a Option<super::trade_ops::Worst>,
) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
    Box::pin(async move {
        let Err(e) = &*res else {
            return false;
        };
        let e = e.clone();
        let prev = Prev {
            q,
            worst: seen.as_ref().map(|w| w.price),
        };
        match branch(core, deal, &e, why, rule, prev).await {
            Outcome::Sold(r) => *res = Ok(*r),
            Outcome::Failed(x) => *res = Err(x),
            // Как sell::sell_or_stuck при «невыгодно»; сделка — копией: продажа после этого завершается.
            Outcome::Stuck(reason) => {
                stuck(core, deal.clone(), reason).await;
                return true;
            }
        }
        false
    })
}

/// Тело ветки: решение и рыночный ордер. Принимает: как run, отказ биржи — `e`. Возвращает: Outcome. Ошибок нет.
pub async fn branch(
    core: &Core,
    deal: &mut Deal,
    e: &OpError,
    why: &str,
    rule: Rule,
    prev: Prev<'_>,
) -> Outcome {
    let head =
        format!("Биржа отклонила лимитный ордер продажи: цена вне разрешённого диапазона ({e})");
    let quote = quote_boxed(core, deal).await;
    let fresh = quote.as_ref().ok().map(|(v, _, _)| Fresh {
        profitable: super::sell::profitable(deal, *v),
        loss_pct: deal.loss_pct_if(*v),
    });
    let act = decide(e.code, e.sent, true, true, rule, marks(deal), fresh);
    // Рыночный ордер: по свежей котировке; у кнопки без неё — по отклонённому лимитному ордеру (given — его цена).
    let (value, q, book, given) = match (act, quote) {
        (Act::Market, Ok((v, q, b))) => (Ok(v), q, b, None),
        (Act::Market, Err(err)) if prev.worst.is_some() => {
            (Err(err.message), prev.q.clone(), None, prev.worst)
        }
        (Act::Stuck, quote) => {
            deal.mark_range_stuck(deal.at.clone());
            let (what, why) = match quote {
                Ok((v, _, _)) => (
                    format!("По стакану продажа здесь невыгодна (оценка выручки {v})"),
                    StuckReason::Unprofitable,
                ),
                Err(err) => (
                    format!("Котировку продажи получить нельзя: {err}"),
                    StuckReason::NoQuote(err.message),
                ),
            };
            let text = format!("{head}. {what} — режим застрявшей монеты (решение 268)");
            note(core, deal, text).await;
            return Outcome::Stuck(why);
        }
        (_, Err(err)) => {
            let m = format!("{e} — рыночная продажа невозможна: нет котировки ({err})");
            let text = format!("{head}. Котировки нет: {err} — рыночного ордера нет (решение 268)");
            note(core, deal, text).await;
            return Outcome::Failed(OpError {
                message: m,
                ..e.clone()
            });
        }
        (_, Ok((v, _, _))) => {
            let text = format!(
                "{head}. {} — рыночного ордера нет (решение 268)",
                unmet(rule, marks(deal), fresh, v)
            );
            note(core, deal, text).await;
            return Outcome::Failed(e.clone());
        }
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
        None => "Свежей котировки нет".to_string(),
    };
    let verdict = met(rule, fresh, why);
    let estimate = match &value {
        Ok(v) => format!("оценка выручки {v} USDT (средняя {})", q.avg_price),
        Err(err) => format!("оценки нет: {err}"),
    };
    let text =
        format!("{head}. {src} — {verdict}: {estimate} — продаём рыночным ордером (решение 268)");
    // Ревью 2: между проверкой условия по котировке и рыночным ордером (предела цены у него нет) — только запись
    // сделки с отметкой (она нужна ДО ордера: один рыночный ордер на площадку). Журнал и Telegram — после ордера.
    deal.mark_range_market(deal.at.clone());
    deal.note(text.clone());
    super::ledger::save(core, deal).await;
    let v = deal.at.clone();
    let (mut seen, mut gas) = (None, Vec::new());
    let res = market_order(core, deal, &q, book, given, &mut seen, &mut gas).await;
    super::deal_gas::charge(core, deal, &v, &mut gas).await;
    journal(core, deal, &text).await;
    // Уведомление — по факту: ордер ушёл на биржу (исполнен или итог неизвестен); не ушёл / отклонён — обычный алерт.
    if res.as_ref().map_or_else(|m| m.sent, |_| true) {
        announce(core, deal, e, &verdict, &value).await;
    }
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

/// Какое условие выполнено на свежей котировке — словами (хронология и Telegram). Принимает: условие продажи, свежую
/// котировку, причину продажи (у режима — в ней его порог и настройка). Ошибок нет.
fn met(rule: Rule, fresh: Option<Fresh>, why: &str) -> String {
    match (rule, fresh) {
        // Процент — тем же расчётом, что в алерте с кнопками (Deal::loss_pct_if → human::sale_now).
        (Rule::Button, Some(f)) => {
            let (k, v) = h::sale_now(f.loss_pct);
            format!("кнопка человека — без проверки порогов; {k}: {v}")
        }
        (Rule::Button, None) => "кнопка человека — без проверки порогов".to_string(),
        (Rule::MaxLoss(max), Some(f)) if !max.is_zero() => format!(
            "условие режима застрявшей монеты выполнено и на свежей котировке — {} (решено продать: {why})",
            h::loss_vs_max(f.loss_pct, max, true)
        ),
        (Rule::MaxLoss(_), _) => {
            format!(
                "условие режима застрявшей монеты «{why}» выполнено и на свежей котировке — продажа выгодна"
            )
        }
        _ => "продажа выгодна".to_string(),
    }
}

/// Почему рыночного ордера нет, хотя котировка есть, — словами. Принимает: условие, отметки, свежую котировку, оценку
/// выручки. Ошибок нет.
fn unmet(rule: Rule, m: Marks, fresh: Option<Fresh>, value: Decimal) -> String {
    match (rule, fresh) {
        _ if m.market => {
            "Рыночный ордер на этой площадке в этой сделке уже отправлялся".to_string()
        }
        (Rule::MaxLoss(max), Some(f)) if !max.is_zero() => format!(
            "По свежей котировке {} (оценка выручки {value})",
            h::loss_vs_max(f.loss_pct, max, false)
        ),
        _ => format!("По свежей котировке продажа невыгодна (оценка выручки {value})"),
    }
}

/// Уведомление deal.sell_market_range (решение 268, п. 7): биржа отклонила ордер по цене, какое условие проверено по
/// стакану, оценка выручки, отправлен рыночный ордер (шлётся ПОСЛЕ ордера — ревью 2). Принимает: ядро, сделку, отказ биржи, условие словами, оценку
/// выручки (Err — оценки нет, причина). Ошибок нет.
async fn announce(
    core: &Core,
    deal: &Deal,
    e: &OpError,
    verdict: &str,
    value: &Result<Decimal, String>,
) {
    let estimate = match value {
        Ok(v) => h::usd(*v),
        Err(err) => format!("оценки нет: {err}"),
    };
    let text = Msg::new(
        h::WARN,
        "Биржа отклонила ордер по цене — отправлен рыночный ордер",
    )
    .card(super::deal_card::card(deal))
    .reason(&e.message)
    .kv("Условие", verdict)
    .kv("Оценка выручки", estimate)
    .kv(
        "Что бот делает",
        "отправил рыночный ордер продажи на биржу (один раз); итог — отдельным сообщением",
    )
    .id(&deal.id)
    .render();
    core.inner
        .deps
        .notify("deal.sell_market_range", Some(&deal.at.0), &text)
        .await;
}

/// Режим застрявшей монеты после ветки (stuck::handle, как sell::sell_or_stuck) — будущее в коробке, собранное вне
/// кадра ветки (глубина стека). Принимает: ядро, сделку, причину. Ошибок нет.
#[inline(never)]
fn stuck<'a>(
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
/// нему; None у кнопки без свежей котировки — тогда `given`: худшая цена отклонённого лимитного ордера), место для
/// худшей цены и газа вне итога. Возвращает и ошибки: как trade_seen.
#[inline(never)]
fn market_order<'a>(
    core: &'a Core,
    deal: &'a Deal,
    q: &'a Quote,
    book: Option<super::exec_book::Book>,
    given: Option<Decimal>,
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
        given,
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
    journal(core, deal, &text).await;
}

/// Строка в журнал core.sell_range. Принимает: ядро, сделку, текст. Ошибок нет.
async fn journal(core: &Core, deal: &Deal, text: &str) {
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
