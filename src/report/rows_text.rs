//! Файл: report/rows_text.rs
//! Что делает: решение 257 — подписи и части строки листа «Сделки» отчёта «Итоги сделок» из учёта сделки:
//! - статус человеческими словами (шаг сделки — core/deal_text.rs step_name; у остановленной и «Беру на себя» — «итог —
//!   оценка»);
//! - площадка итога по правилу владельца (Тигран 11.11): биржа ↔ DEX — бирже; биржа ↔ биржа — бирже ПОКУПКИ;
//!   DEX ↔ DEX — DEX (бот в итоге дня по площадкам, решение 173 / core/daily_venue.rs, биржа ↔ биржа относит к обеим
//!   биржам — расхождение отмечено в книге);
//! - состояние возврата USDT по частям (core/usdt_return_state.rs) и его затраты (core/deal_return_cost.rs known);
//! - части плана из строк расчёта (core/costs.rs → Calc.costs в Deal.plan): запас на движение цены (PriceMoveReserve) и
//!   затраты возврата USDT (строки этапа «возврат USDT»), только вычитаемые (не в цене, не в количестве), как costs_usdt;
//! - подписи видов затрат (CostKind) для колонок разбивки.
//!
//! Кто вызывает: report/rows.rs.
//! От чего зависит: core/deal.rs, core/deal_text.rs, core/usdt_return_state.rs, core/usdt_return_parts.rs,
//! core/deal_return_cost.rs, core/types (CostKind), notify/human.rs (venue, money).
//! Настройки: нет.

use std::collections::BTreeSet;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::deal::{Deal, Step};
use crate::core::types::CostKind;
use crate::core::usdt_return_state::UsdtReturn;
use crate::notify::human::{money, venue};

/// Статус сделки для человека. Ошибок нет.
pub fn status(d: &Deal) -> String {
    let s = crate::core::deal_text::step_name(d.step);
    match d.step {
        Step::Halted | Step::TakenOver => format!("{s} — итог: оценка позиции, не факт"),
        _ => s.to_string(),
    }
}

/// Площадка итога сделки по правилу владельца (см. шапку). Принимает: сделку, id площадок DEX. Возвращает: id.
pub fn owner_venue(d: &Deal, dex: &BTreeSet<String>) -> String {
    let (b, s) = (&d.direction.buy_on.0, &d.direction.sell_on.0);
    match (dex.contains(b), dex.contains(s)) {
        (false, _) => b.clone(),
        (true, false) => s.clone(),
        (true, true) => b.clone(),
    }
}

/// Состояние одной части возврата USDT. Ошибок нет.
fn part(p: &UsdtReturn) -> String {
    match p {
        UsdtReturn::Pending { amount, from, to } => format!(
            "идёт: {} USDT {} → {}",
            money(*amount),
            venue(&from.0),
            venue(&to.0)
        ),
        UsdtReturn::Retrying { amount, tries, .. } => {
            format!("ждёт повтора ({tries}): {} USDT", money(*amount))
        }
        UsdtReturn::Done {
            amount, credited, ..
        } => format!(
            "вернулись: {} USDT, зачислено {}",
            money(*amount),
            money(*credited)
        ),
        UsdtReturn::NotReturned { amount, at, .. } => format!(
            "не вернулись: {} USDT лежат на {}",
            money(*amount),
            venue(&at.0)
        ),
        UsdtReturn::Manual {
            amount, credited, ..
        } => match credited {
            Some(c) => format!(
                "перегнано вручную: {} USDT, зачислено {}",
                money(*amount),
                money(*c)
            ),
            None => format!(
                "перегнано вручную: {} USDT, зачислено — неизвестно (затраты не в итоге)",
                money(*amount)
            ),
        },
    }
}

/// Состояние возврата USDT сделки: части через «; »; возврата нет — причина или «не нужен». Ошибок нет.
pub fn usdt_return(d: &Deal) -> String {
    let parts = crate::core::usdt_return_parts::all(d);
    if parts.is_empty() {
        return match &d.usdt_return_skip {
            Some(why) => format!("нет: {why}"),
            None => "нет возврата".into(),
        };
    }
    parts.into_iter().map(part).collect::<Vec<_>>().join("; ")
}

/// Части плана: (запас на движение цены, затраты возврата USDT) — сумма USDT вычитаемых строк Calc.costs.
/// Принимает: план сделки (JSON). Возвращает: None — у плана нет строк расчёта (цикл DEX ↔ DEX, старая запись).
pub fn plan_parts(plan: &Value) -> Option<(Decimal, Decimal)> {
    let costs = plan.get("costs")?.as_array()?;
    let (mut reserve, mut ret) = (Decimal::ZERO, Decimal::ZERO);
    for c in costs {
        let flag = |k: &str| c.get(k).and_then(Value::as_bool).unwrap_or(false);
        if flag("in_price") || flag("in_qty") {
            continue;
        }
        let usdt = c
            .get("usdt")
            .and_then(|u| serde_json::from_value::<Decimal>(u.clone()).ok())
            .unwrap_or_default();
        let kind = c
            .get("kind")
            .and_then(|k| serde_json::from_value::<CostKind>(k.clone()).ok());
        if kind == Some(CostKind::PriceMoveReserve) {
            reserve += usdt;
        } else if c.get("stage").and_then(Value::as_str)
            == Some(crate::core::costs_types::RETURN_STAGE)
        {
            ret += usdt;
        }
    }
    Some((reserve, ret))
}

/// Подпись вида затраты для колонки разбивки. Ошибок нет.
pub fn cost_label(k: CostKind) -> &'static str {
    match k {
        CostKind::TradeFee => "Торговые комиссии, USDT",
        CostKind::PoolFee => "Комиссия пула, USDT",
        CostKind::StepRemainder => "Остаток шага, USDT",
        CostKind::Gas => "Газ обменов, USDT",
        CostKind::ApproveGas => "Газ approve, USDT",
        CostKind::TransferGas => "Газ перевода монеты, USDT",
        CostKind::WrapGas => "Газ обёртки BNB, USDT",
        CostKind::BuyTax => "Налог токена (покупка), USDT",
        CostKind::SellTax => "Налог токена (продажа), USDT",
        CostKind::TransferTax => "Налог токена (перевод), USDT",
        CostKind::TransferShortfall => "Недостача перевода, USDT",
        CostKind::HookImpact => "Влияние крючка пула, USDT",
        CostKind::ChannelTip => "Чаевые канала и газ их перевода, USDT",
        CostKind::WithdrawFee => "Комиссия вывода, USDT",
        CostKind::UsdtReturn => "Возврат USDT (затраты), USDT",
        CostKind::PriceMoveReserve => "Запас на движение цены, USDT",
    }
}
