//! Файл: core/credit_fact.rs
//! Что делает: решение 413 (t198) — «фактическое количество монет сделки на площадке»: запись сделки о зачислении
//! перевода на биржу (Deal.credit). Запись депозита биржи (Transfer.amount по хешу перевода сделки) — привязка депозита
//! к сделке и ВЕРХНЯЯ граница; количество, которым сделка владеет, подтверждается фактом баланса (credit_apply.rs).
//!
//! Состояния:
//! - share = None — «ждёт доли»: количество сделки = запись депозита, факт ещё не прочитан (суммируется в распределение
//!   credit_split.rs как ждущая; в сверке балансов — нижней оценкой запись × (1 − ожидаемое удержание),
//!   reconcile_confirm.rs; подтверждается перед продажей / выводом или на круге сверки);
//! - share = Some — подтверждено: Basis::Balance — по факту баланса; Record — только у записей до ревью t198 (с ревью
//!   С1 любое зачисление на биржу ждёт доли). Оценки без факта нет (повторное ревью, Б-3: баланс не прочитан — не
//!   продаём).
//!
//! - rest = Some — четвёртое ревью t198 (С-3, С-5): доля подтверждена не вся — «не присвоено, перечитать» (на счёте
//!   были другие депозиты монеты после снимка или сделка в пути / уходящая): сделка ведётся долей, а следующее чтение
//!   факта (перед продажей / выводом) перераспределяет количество + остаток (reopen) и добирает его.
//! Поле reread — только в памяти: количество, на котором продажа получила отказ «мало средств» (sell_fact.rs).
//! Поля tx и deposits — хеш вывода этой сделки и хеши депозитов монеты на счёт до вывода (credit_base.rs) — для
//! проверки «других депозитов после снимка нет» (С-3).
//! Кто вызывает: core/fills.rs (on_credit — после зачисления перевода сделки), credit_apply.rs, sell_fact.rs,
//! remain_others.rs, reconcile_confirm.rs, tests.
//! От чего зависит: credit_base.rs (посторонние до вывода, по хешу), trade_ops.rs (вид площадки),
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
    /// Когда снят снимок посторонних (повторное ревью, С-3: в распределении — самый свежий).
    #[serde(default)]
    pub outsider_at: Option<DateTime<Utc>>,
    /// Подтверждённое количество; None — ждёт доли.
    #[serde(default)]
    pub share: Option<Decimal>,
    /// Третье ревью (М-5): основание, которого нет в этой сборке (например, Estimate ранних сборок t198), при чтении —
    /// None (подтверждено учётом, если share есть; иначе ждёт доли), запись не падает.
    #[serde(default, deserialize_with = "basis_lenient")]
    pub basis: Option<Basis>,
    pub at: DateTime<Utc>,
    /// Четвёртое ревью (С-3, С-5): остаток записи «не присвоено, перечитать»; None — доля окончательна.
    #[serde(default)]
    pub rest: Option<Decimal>,
    /// Хеш вывода этой сделки (депозит этой сделки в истории биржи).
    #[serde(default)]
    pub tx: Option<String>,
    /// Хеши депозитов монеты на счёт до вывода (снимок credit_base.rs); None — не сняты (элемент None — без хеша).
    #[serde(default)]
    pub deposits: Option<Vec<Option<String>>>,
    /// Только в памяти: количество, на котором продажа получила отказ «мало средств» (sell_fact.rs).
    #[serde(skip)]
    pub reread: Option<Decimal>,
}

/// Что делает: чтение основания без отказа на незнакомом значении (М-5). Принимает: разборщик. Возвращает: основание
/// или None. Ошибки: только формата JSON.
fn basis_lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Basis>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}

/// Что делает: после зачисления перевода сделки (fills.rs transfer): запись зачисления. Ревью t198 (С1): любая
/// биржа-получатель — «ждёт доли» (факт — перед продажей или выводом, sell_fact.rs; на круге сверки — reconcile.rs),
/// снимок посторонних — по хешу вывода (credit_base.rs). Получатель DEX — записи нет (кошелёк получает по событию
/// Transfer, решения 151, 222). Принимает: ядро, сделку (qty уже = запись), получателя, хеш вывода. Ошибок нет.
pub fn on_credit(core: &Core, deal: &mut Deal, to: &VenueId, tx: Option<&str>) {
    let snap = super::credit_base::take(tx);
    let deposits = super::credit_base::take_deposits(tx);
    if super::trade_ops::kind(core, to) != Ok(VenueKind::Cex) {
        deal.credit = None;
        return;
    }
    deal.credit = Some(Box::new(CreditFact {
        venue: to.clone(),
        record: deal.qty,
        outsider: snap.map(|x| x.0),
        outsider_at: snap.map(|x| x.1),
        share: None,
        basis: None,
        at: chrono::Utc::now(),
        rest: None,
        tx: tx.map(str::to_string),
        deposits,
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
        c.rest = None;
    }
}

/// Что делает: четвёртое ревью (С-3, С-5) — доля подтверждена не вся: остаток записи «не присвоено, перечитать».
/// Принимает: сделку, остаток (≤ 0 — доля окончательна). Ошибок нет.
pub fn set_rest(deal: &mut Deal, rest: Decimal) {
    if let Some(c) = deal.credit.as_deref_mut() {
        c.rest = (rest > Decimal::ZERO).then_some(rest);
    }
}

/// Что делает: следующее чтение факта сделки с остатком (С-3, С-5) — снова «ждёт доли» с записью = количество +
/// остаток; замер удержания по ней не пишется (снимка посторонних нет — замер был при первой доле). Принимает: сделку.
/// Возвращает: да — сделка снова ждёт доли. Ошибок нет.
pub fn reopen(deal: &mut Deal) -> bool {
    let at = deal.at.clone();
    let qty = deal.qty;
    let Some(c) = deal.credit.as_deref_mut().filter(|c| c.venue == at) else {
        return false;
    };
    let Some(rest) = c.rest.take() else {
        return false;
    };
    c.record = qty + rest;
    c.share = None;
    c.basis = None;
    c.outsider = None;
    c.outsider_at = None;
    true
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
            outsider_at: None,
            share: Some(qty),
            basis: None,
            at: chrono::Utc::now(),
            rest: None,
            tx: None,
            deposits: None,
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
