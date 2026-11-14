//! Файл: core/sell_range.rs
//! Что делает: решение 268 (Тигран 14.11.2026; живой случай 13.11 21:13 UTC, три сделки FAR bsc_dex → mexc: в стакане
//! покупатели на ~20% выше цены покупки, а MEXC отклонил лимитный ордер продажи — {"msg":"Order price exceeds allowed
//! range","code":30087}: коридор цены от последней сделки; сделка ушла в «ждём решения человека», через 5 минут цена
//! ушла, итог −3.29 USDT). Биржа отклонила ЛИМИТНЫЙ ордер продажи сделки кодом шлюза PriceOutOfRange (ничего не
//! продано) — сразу свежая котировка продажи (trade_ops::deal_quote → exec_book.rs: стакан из памяти адаптера, если
//! свежий, иначе один снимок REST — решение 238) и на ней пересчитывается ТО ЖЕ условие, по которому продажа была
//! решена (Rule); выполнено — рыночный ордер биржи (контракт trade_market_sell), его факт или ошибку дальше разбирает
//! sell.rs тем же кодом, что у лимитного:
//! - обычная продажа (после зачисления, после перегона — sell::sell_or_stuck / sell_after_relay; Rule::Entry): выгодно
//!   (sell::profitable — итог сделки ≥ 0, решение 47) — рыночный ордер; невыгодно или котировки нет — режим застрявшей
//!   монеты (stuck::handle), не больше одного раза на площадку в сделке;
//! - режим застрявшей монеты сам решил продать здесь и его ордер упёрся в коридор (Тигран 14.11: «кидаем по маркету,
//!   но считаем: кидать по маркету будет меньше убытка, чем наша настройка, или нет — тот же механизм»): «Порог убытка»
//!   и продажа здесь в «Перегоне» (Rule::MaxLoss(порог режима)) — убыток по свежей котировке не выше того же порога —
//!   рыночный ордер; «Ожидание: стало выгодно» (порог 0) — выгодно — рыночный ордер; условие не выполнено или котировки
//!   нет — алерт человеку, как раньше (второго круга режима нет);
//! - продажа без порога («Ожидание» по истечении stuck.wait_ms, продажа после частичной покупки; Rule::NoThreshold) —
//!   рыночного ордера нет: алерт, как раньше (нет порога — нет автоматического рыночного);
//! - кнопка человека («Продать сейчас», продажи «Продать и перегнать»; sell::BUTTON, решение 235; Rule::Button) —
//!   рыночный ордер без проверки порогов; свежая котировка берётся для текста (оценка выручки и процент «при продаже
//!   сейчас» — тем же расчётом, что в алерте с кнопками); котировки нет — рыночный ордер всё равно уходит, в тексте
//!   «оценки нет: причина»;
//! - площадка рыночную продажу не умеет (NotApplicable) — алерт, как раньше, с причиной.
//! Круга нет: рыночный ордер автоматически уходит не больше одного раза на площадку в сделке (Deal.range_market), уход
//! в режим из-за коридора — не больше одного раза на площадку (Deal.range_stuck); режим повторно в режим не уходит;
//! всё остальное — алерт человеку. Нажатие кнопки человека отметки очищает (свой один рыночный повтор). Рыночный ордер
//! код PriceOutOfRange не отдаёт никогда (адаптер; gateway/errors.rs no_price_range). Покупок рыночным ордером нет.
//! Настройка выключена или не задана — ветки нет: PriceOutOfRange ведёт себя как прежний OrderRejected у продажи (алерт).
//! Здесь — решение (чистые applies / possible / decide, wanted по настройке); исполнение ветки — sell_range_run.rs.
//! Кто вызывает: core/sell.rs (rule_of, wanted — после ордера продажи), core/sell_range_run.rs (decide, Outcome).
//! От чего зависит: trade_ops.rs (kind), deal.rs, op_error.rs, stuck_reason.rs, remeasure.rs (Guard), gateway/errors.rs.
//! Настройки: sell.market_on_price_range (вся система и площадка — та, где продажа).

use rust_decimal::Decimal;

use crate::core::types::TradeResult;
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;
use super::remeasure::Guard;
use super::stuck_reason::StuckReason;

/// Настройка-включатель.
pub const ON: &str = "sell.market_on_price_range";

/// Условие, по которому продажа была решена, — оно же пересчитывается на свежей котировке после отказа по коридору.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Обычная продажа (после зачисления, после перегона): условие «выгодно»; невыгодно — режим застрявшей монеты.
    Entry,
    /// Режим застрявшей монеты решил продать здесь: убыток не выше порога, % (stuck.max_loss_pct — «Порог убытка»,
    /// stuck.relay_max_loss_pct — продажа здесь в «Перегоне»; 0 — «Ожидание: стало выгодно», условие «выгодно»).
    MaxLoss(Decimal),
    /// Продажа без порога («Ожидание» по истечении, частичная покупка) — рыночного ордера нет.
    NoThreshold,
    /// Кнопка человека — без проверки условия.
    Button,
}

/// Отметки сделки на площадке продажи: рыночный ордер уже отправлялся; из-за коридора уже уходили в режим.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Marks {
    pub market: bool,
    pub stuck: bool,
}

/// Свежая котировка продажи глазами сделки: выгодно ли (итог ≥ 0) и убыток, % (Deal::loss_pct_if).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fresh {
    pub profitable: bool,
    pub loss_pct: Decimal,
}

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

/// Что делает: условие продажи по её причине и порогу пере-замера, с которыми она запущена (sell::market_guarded):
/// кнопка — Button; порог убытка режима (Guard::MaxLoss — stuck.rs «Порог убытка», «Ожидание: стало выгодно»,
/// relay_here.rs) — MaxLoss; прочее (Guard::Open — «Ожидание» по истечении, частичная покупка; Guard::Off) —
/// NoThreshold. Обычная продажа (sell_or_stuck) условие Entry задаёт сама. Принимает: причину, порог. Ошибок нет.
pub fn rule_of(why: &str, g: Guard) -> Rule {
    match g {
        _ if why == super::sell::BUTTON => Rule::Button,
        Guard::MaxLoss(max) => Rule::MaxLoss(max),
        Guard::Any | Guard::Off | Guard::Open(_) => Rule::NoThreshold,
    }
}

/// Что делает: относится ли отказ продажи к ветке решения 268. Принимает: код ошибки продажи, ушёл ли ордер / транзакция
/// (OpError.sent), площадка — биржа, включена ли настройка. Возвращает: да / нет. Ошибок нет.
pub fn applies(code: Option<ErrorCode>, sent: bool, cex: bool, on: bool) -> bool {
    code == Some(ErrorCode::PriceOutOfRange) && !sent && cex && on
}

/// Что делает: может ли ветка ещё что-то сделать на этой площадке (иначе котировка не берётся — сразу как раньше):
/// рыночный ордер ещё не отправлялся, либо (обычная продажа) в режим из-за коридора ещё не уходили. Ошибок нет.
pub fn possible(rule: Rule, m: Marks) -> bool {
    match rule {
        Rule::NoThreshold => false,
        Rule::Button | Rule::MaxLoss(_) => !m.market,
        Rule::Entry => !m.market || !m.stuck,
    }
}

/// Что делает: чистое решение ветки. Принимает: как applies; условие продажи; отметки сделки на площадке; свежую
/// котировку (None — котировки нет). Возвращает: Act. Ошибок нет.
pub fn decide(
    code: Option<ErrorCode>,
    sent: bool,
    cex: bool,
    on: bool,
    rule: Rule,
    m: Marks,
    fresh: Option<Fresh>,
) -> Act {
    if !applies(code, sent, cex, on) || !possible(rule, m) {
        return Act::Keep;
    }
    let market = |ok: bool| {
        if ok && !m.market {
            Act::Market
        } else {
            Act::Keep
        }
    };
    match (rule, fresh) {
        (Rule::NoThreshold, _) => Act::Keep,
        // Кнопка: человек уже решил — без проверки условия; свежая котировка — только для оценки в тексте (нет её —
        // рыночный ордер всё равно уходит, Тигран 14.11).
        (Rule::Button, _) => market(true),
        // Режим застрявшей монеты: то же условие режима на свежей котировке; не выполнено / нет котировки — алерт.
        (Rule::MaxLoss(max), Some(f)) => market(holds(max, f)),
        (Rule::MaxLoss(_), None) => Act::Keep,
        // Обычная продажа: выгодно — рыночный (если ещё не отправлялся); невыгодно / нет котировки — режим, один раз.
        (Rule::Entry, Some(f)) if f.profitable => market(true),
        (Rule::Entry, _) if !m.stuck => Act::Stuck,
        (Rule::Entry, _) => Act::Keep,
    }
}

/// Условие режима на свежей котировке: порог 0 — «выгодно» (sell::profitable, как «Ожидание: стало выгодно»); иначе
/// убыток не выше порога (как stuck.rs loss_limit и relay_here::decide). Ошибок нет.
pub fn holds(max: Decimal, f: Fresh) -> bool {
    if max.is_zero() {
        f.profitable
    } else {
        f.loss_pct <= max
    }
}

/// Отметки сделки на площадке, где лежат её монеты. Ошибок нет.
pub fn marks(deal: &Deal) -> Marks {
    Marks {
        market: deal.range_market.contains(&deal.at),
        stuck: deal.range_stuck.contains(&deal.at),
    }
}

/// Что делает: идёт ли отказ продажи сделки в ветку решения 268 (код, вид площадки, настройка площадки продажи, условие
/// продажи и отметки сделки). Принимает: ядро, сделку, ошибку продажи, условие продажи. Ошибок нет: настройка не задана
/// или не читается — нет.
pub fn wanted(core: &Core, deal: &Deal, e: &OpError, rule: Rule) -> bool {
    applies(e.code, e.sent, is_cex(core, deal), on(core, deal)) && possible(rule, marks(deal))
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
