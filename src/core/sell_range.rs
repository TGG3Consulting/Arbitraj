//! Файл: core/sell_range.rs
//! Что делает: решение 268 (Тигран 14.11.2026; живой случай 13.11 21:13 UTC, три сделки FAR bsc_dex → mexc: в стакане
//! покупатели на ~20% выше цены покупки, а MEXC отклонил лимитный ордер продажи — {"msg":"Order price exceeds allowed
//! range","code":30087}: коридор цены от последней сделки; сделка ушла в «ждём решения человека», через 5 минут цена
//! ушла, итог −3.29 USDT). Биржа отклонила ЛИМИТНЫЙ ордер продажи сделки кодом шлюза PriceOutOfRange — один раз на
//! сделку (Deal.price_range_tried):
//! 1. свежая котировка продажи существующим кодом (trade_ops::deal_quote → exec_book.rs: стакан из памяти адаптера,
//!    если свежий, иначе снимок REST — решение 238);
//! 2. продажа выгодна (sell::profitable — итог сделки ≥ 0, решение 47) — рыночный ордер биржи (контракт
//!    trade_market_sell, через trade_ops::trade_seen с признаком market); его факт или ошибку дальше разбирает sell.rs
//!    тем же кодом, что у лимитного ордера (исполнение, частичное исполнение, пыль, возврат USDT, сверка с планом,
//!    «исход неизвестен», алерт);
//! 3. невыгодно или котировки нет — режим застрявшей монеты (stuck::handle, как sell::sell_or_stuck);
//! 4. кнопка человека (sell::BUTTON, решение 235 — человек сам решил продать) — рыночный ордер без проверки «выгодно»;
//!    нет котировки — алерт, как раньше;
//! 5. площадка рыночную продажу не умеет (NotApplicable) — алерт, как раньше, с причиной.
//! Второй отказ PriceOutOfRange у той же сделки (после рыночного ордера или из режима застрявшей монеты) сюда не идёт
//! — алерт человеку, как раньше (sell_unsent.rs → Retry::No): повтора по кругу нет. Рыночный ордер код PriceOutOfRange
//! не отдаёт никогда (адаптер; gateway/errors.rs no_price_range). Покупок рыночным ордером нет (решение 262 не тронуто).
//! Настройка выключена или не задана — ветки нет: PriceOutOfRange ведёт себя как прежний OrderRejected у продажи (алерт).
//! Кто вызывает: core/sell.rs (sell — после ордера продажи).
//! От чего зависит: trade_ops.rs (deal_quote, trade_seen, kind), sell.rs (profitable, BUTTON), deal_gas.rs, ledger.rs,
//! stuck_reason.rs, op_error.rs, deps.rs, gateway/errors.rs.
//! Настройки: sell.market_on_price_range (вся система и площадка — та, где продажа).

use crate::core::types::{Side, TradeResult};
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;
use super::stuck_reason::StuckReason;

/// Настройка-включатель.
pub const ON: &str = "sell.market_on_price_range";

/// Что делать с отказом продажи (чистое решение).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Ветки решения 268 нет — дальше как раньше (sell_unsent.rs: алерт человеку).
    Keep,
    /// Рыночный ордер продажи.
    Market,
    /// Режим застрявшей монеты.
    Stuck,
}

/// Итог ветки для sell.rs.
pub enum Outcome {
    /// Рыночный ордер исполнился (полностью или частично) — факт.
    Sold(TradeResult),
    /// Ошибка, с которой sell.rs идёт дальше как с ошибкой продажи (ошибка рыночного ордера; исходный отказ с причиной,
    /// почему рыночной продажи нет).
    Failed(OpError),
    /// Режим застрявшей монеты с причиной.
    Stuck(StuckReason),
}

/// Что делает: относится ли отказ продажи к ветке решения 268. Принимает: код ошибки продажи, ушёл ли ордер / транзакция
/// (OpError.sent), площадка — биржа, включена ли настройка, была ли ветка у сделки. Возвращает: да / нет. Ошибок нет.
pub fn applies(code: Option<ErrorCode>, sent: bool, cex: bool, on: bool, done: bool) -> bool {
    code == Some(ErrorCode::PriceOutOfRange) && !sent && cex && on && !done
}

/// Что делает: чистое решение ветки. Принимает: как applies; кнопка ли человека; выгодна ли продажа по свежей
/// котировке (None — котировки нет). Возвращает: Act. Ошибок нет.
pub fn decide(
    code: Option<ErrorCode>,
    sent: bool,
    cex: bool,
    on: bool,
    done: bool,
    button: bool,
    profitable: Option<bool>,
) -> Act {
    if !applies(code, sent, cex, on, done) {
        return Act::Keep;
    }
    match (button, profitable) {
        // Кнопка: человек уже решил — без проверки «выгодно»; без котировки ордер не собрать — как раньше.
        (true, Some(_)) => Act::Market,
        (true, None) => Act::Keep,
        (false, Some(true)) => Act::Market,
        (false, _) => Act::Stuck,
    }
}

/// Что делает: идёт ли отказ продажи сделки в ветку решения 268 (по настройке площадки продажи, виду площадки и
/// отметке сделки). Принимает: ядро, сделку, ошибку продажи. Ошибок нет: настройка не задана или не читается — нет.
pub fn wanted(core: &Core, deal: &Deal, e: &OpError) -> bool {
    applies(
        e.code,
        e.sent,
        is_cex(core, deal),
        on(core, deal),
        deal.price_range_tried,
    )
}

/// Площадка, где лежат монеты сделки, — биржа. Ошибок нет (площадки нет в реестре — нет).
fn is_cex(core: &Core, deal: &Deal) -> bool {
    super::trade_ops::kind(core, &deal.at) == Ok(VenueKind::Cex)
}

/// Настройка sell.market_on_price_range площадки продажи (или общей области). Не задана — нет. Ошибок нет.
fn on(core: &Core, deal: &Deal) -> bool {
    let at = At {
        venue: Some(&deal.at.0),
        coin: None,
    };
    core.inner.deps.settings.flag(ON, at).unwrap_or(false)
}

/// Что делает: ветка решения 268 (вызывать только при wanted): отметка сделки, свежая котировка, решение, строка
/// хронологии и журнал core.sell_range, рыночный ордер. Принимает: ядро, сделку (отметка, хронология и газ пишутся в
/// неё), ошибку лимитного ордера, причину продажи (sell::BUTTON — кнопка). Возвращает: Outcome. Ошибок нет.
pub async fn run(core: &Core, deal: &mut Deal, e: &OpError, why: &str) -> Outcome {
    deal.price_range_tried = true;
    let button = why == super::sell::BUTTON;
    let head =
        format!("Биржа отклонила лимитный ордер продажи: цена вне разрешённого диапазона ({e})");
    let quote = super::trade_ops::deal_quote(core, deal).await;
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
    let (v, a) = (deal.at.clone(), deal.direction.asset.clone());
    let (mut seen, mut gas) = (None, Vec::new());
    let res = super::trade_ops::trade_seen(
        core,
        &deal.id,
        &v,
        &a,
        Side::Sell,
        deal.qty,
        q.route.clone(),
        q.avg_price,
        None,
        None,
        book,
        &mut seen,
        Some(&mut gas),
        None,
        true,
    )
    .await;
    super::deal_gas::charge(core, deal, &v, &mut gas).await;
    match res {
        Ok(r) => {
            deal.note(format!(
                "Рыночный ордер продажи исполнен: {} по средней {} (решение 268)",
                r.filled_qty, r.avg_price
            ));
            Outcome::Sold(r)
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
