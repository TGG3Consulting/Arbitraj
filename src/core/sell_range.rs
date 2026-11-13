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
//!
//! Второй отказ PriceOutOfRange у той же сделки (после рыночного ордера или из режима застрявшей монеты) сюда не идёт
//! — алерт человеку, как раньше (sell_unsent.rs → Retry::No): повтора по кругу нет. Рыночный ордер код PriceOutOfRange
//! не отдаёт никогда (адаптер; gateway/errors.rs no_price_range). Покупок рыночным ордером нет (решение 262 не тронуто).
//! Настройка выключена или не задана — ветки нет: PriceOutOfRange ведёт себя как прежний OrderRejected у продажи (алерт).
//! Здесь — решение (чистые applies / decide, wanted по настройке); исполнение ветки — sell_range_run.rs.
//! Кто вызывает: core/sell.rs (wanted — после ордера продажи), core/sell_range_run.rs (decide, Outcome).
//! От чего зависит: trade_ops.rs (kind), deal.rs, op_error.rs, stuck_reason.rs, gateway/errors.rs, settings.
//! Настройки: sell.market_on_price_range (вся система и площадка — та, где продажа).

use crate::core::types::TradeResult;
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;

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
    Sold(Box<TradeResult>),
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
