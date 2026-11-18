//! Файл: core/credit_fact.rs
//! Что делает: решение 413 (t198) — «фактическое количество монет сделки на площадке»: запись сделки о зачислении
//! перевода на биржу (Deal.credit). Запись депозита биржи (Transfer.amount по хешу перевода сделки) — привязка депозита
//! к сделке и ВЕРХНЯЯ граница; количество, которым сделка владеет, подтверждается фактом баланса (credit_apply.rs).
//!
//! Состояния:
//! - share = None — «ждёт доли»: количество сделки = запись депозита, факт ещё не прочитан (суммируется в распределение
//!   credit_split.rs как ждущая, в сверке балансов — не участвует: учёт не доказан, reconcile_confirm.rs);
//! - share = Some — подтверждено: Basis::Balance — по факту баланса; Record — по записи (монета без ожидаемого
//!   удержания: налог токена 0 и замер 0 / нет — deposit_hold.rs suspect = нет; отказ «мало средств» на продаже вернёт
//!   её в «ждёт доли», sell_fact.rs); Estimate — баланс не читался, оценка «запись × (1 − ожидаемое удержание)»
//!   (не больше доли сделки при удержании не меньше ожидаемого).
//!
//! Поле reread — только в памяти: количество, на котором продажа получила отказ «мало средств» (sell_fact.rs).
//! Кто вызывает: core/fills.rs (on_credit — после зачисления перевода сделки), credit_apply.rs, sell_fact.rs,
//! remain_others.rs, reconcile_confirm.rs, tests.
//! От чего зависит: credit_base.rs (посторонние до вывода), deposit_hold.rs (suspect), trade_ops.rs (вид площадки),
//! deal.rs, core/types.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;

use super::deal::{Deal, Step};
use super::engine::Core;

/// Чем подтверждено количество сделки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Basis {
    /// Запись депозита (удержания не ожидается).
    Record,
    /// Факт баланса счёта (распределение credit_split.rs).
    Balance,
    /// Оценка: баланс не читался, запись × (1 − ожидаемое удержание).
    Estimate,
}

/// Зачисление перевода сделки на биржу.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditFact {
    /// Биржа зачисления.
    pub venue: VenueId,
    /// Запись депозита биржи.
    pub record: Decimal,
    /// Посторонние монеты на счёте перед выводом (credit_base.rs); None — не сняты.
    #[serde(default)]
    pub outsider: Option<Decimal>,
    /// Подтверждённое количество; None — ждёт доли.
    #[serde(default)]
    pub share: Option<Decimal>,
    #[serde(default)]
    pub basis: Option<Basis>,
    pub at: DateTime<Utc>,
    /// Только в памяти: количество, на котором продажа получила отказ «мало средств» (sell_fact.rs).
    #[serde(skip)]
    pub reread: Option<Decimal>,
}

/// Что делает: после зачисления перевода сделки (fills.rs transfer): запись зачисления. Биржа-получатель, монета с
/// ожидаемым удержанием или неизмеренным налогом — «ждёт доли» (факт — перед продажей); иначе — подтверждено записью.
/// Получатель DEX — записи нет (кошелёк получает по событию Transfer, решения 151, 222). Принимает: ядро, сделку (qty
/// уже = запись), получателя. Ошибок нет.
pub fn on_credit(core: &Core, deal: &mut Deal, to: &VenueId) {
    if super::trade_ops::kind(core, to) != Ok(VenueKind::Cex) {
        deal.credit = None;
        return;
    }
    let pending = super::deposit_hold::suspect(core, to, &deal.direction.asset);
    deal.credit = Some(Box::new(CreditFact {
        venue: to.clone(),
        record: deal.qty,
        outsider: super::credit_base::take(&deal.id),
        share: (!pending).then_some(deal.qty),
        basis: (!pending).then_some(Basis::Record),
        at: chrono::Utc::now(),
        reread: None,
    }));
}

/// Сделка ждёт доли на площадке, где сейчас её монеты (не шаг «перевод»). Ошибок нет.
pub fn pending(d: &Deal) -> bool {
    d.step != Step::Transfer
        && d.credit
            .as_ref()
            .is_some_and(|c| c.venue == d.at && c.share.is_none())
}

/// Запись зачисления на площадке, где монеты сейчас. Ошибок нет.
pub fn here(d: &Deal) -> Option<&CreditFact> {
    d.credit.as_deref().filter(|c| c.venue == d.at)
}

/// Что делает: подтверждает количество сделки. Принимает: сделку, количество, основание. Ошибок нет.
pub fn confirm(deal: &mut Deal, qty: Decimal, basis: Basis) {
    deal.qty = qty.max(Decimal::ZERO);
    if let Some(c) = deal.credit.as_deref_mut() {
        c.share = Some(deal.qty);
        c.basis = Some(basis);
    }
}

/// Что делает: отказ «мало средств» на продаже количества `tried` (sell_fact.rs): запомнить и, если количество было
/// подтверждено только записью и ещё не продавалось, вернуть в «ждёт доли» (факт перераспределит). Ошибок нет.
pub fn mark_reread(deal: &mut Deal, tried: Decimal) {
    let (at, qty) = (deal.at.clone(), deal.qty);
    // Записи зачисления на этой площадке нет (куплено здесь, или запись прежней площадки) — новая, подтверждённая учётом.
    if !deal.credit.as_ref().is_some_and(|c| c.venue == at) {
        deal.credit = Some(Box::new(CreditFact {
            venue: at,
            record: qty,
            outsider: None,
            share: Some(qty),
            basis: None,
            at: chrono::Utc::now(),
            reread: None,
        }));
    }
    let Some(c) = deal.credit.as_deref_mut() else {
        return;
    };
    if c.basis == Some(Basis::Record) && c.share == Some(c.record) && qty == c.record {
        c.share = None;
        c.basis = None;
    }
    c.reread = Some(tried);
}

/// Что делает: снимает отметку отказа (продажа прошла — fills.rs trade). Ошибок нет.
pub fn clear_reread(deal: &mut Deal) {
    if let Some(c) = deal.credit.as_deref_mut() {
        c.reread = None;
    }
}
