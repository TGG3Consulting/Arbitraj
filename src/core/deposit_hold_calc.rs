//! Файл: core/deposit_hold_calc.rs
//! Что делает: решение 413 (t198) — ожидаемое удержание биржи при зачислении и строка затрат DepositHold (вынесено из
//! deposit_hold.rs, ревью t198). Ожидаемое удержание — в расчёт прибыли и количества СЛЕДУЮЩИХ сделок (шаг «перевод»,
//! в монете: приход на B меньше) и в распределение долей:
//! - получатель DEX или нативная монета — 0 (кошелёк получает по событию Transfer = зачислено, решения 151/222);
//! - замер есть — его мера (deposit_hold.rs: наибольший % последних выборок);
//! - замера нет — налог перевода токена (transfer_tax::measured, проверки налогов DEX): у WKC удержание равно ему —
//!   консервативно, чтобы первая сделка не ушла в минус так, как WKC; налога нет (0) — 0; не измерен — 0 (такие
//!   сделки не проходят расчёт: transfer_tax.rs).
//!
//! Ревью t198: налог токена — без копии списка площадок на каждый вызов (горячий путь расчёта).
//! Кто вызывает: core/costs_source.rs, core/relay_value.rs (add), core/credit_apply.rs, credit_base.rs, credit_alert.rs,
//! credit_text.rs (expected_pct), тесты (expected_of, line).
//! От чего зависит: deposit_hold.rs (get), transfer_tax.rs, native.rs, trade_ops.rs, convert.rs, engine.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{Asset, Cost, CostKind, VenueId};
use crate::gateway::contract::VenueKind;

use super::engine::{Core, rd};

/// Что делает: ожидаемое удержание по чистым данным (правило — шапка). Принимает: получатель — биржа, монета нативная,
/// замер (%), налог перевода токена (%; None — не измерен). Возвращает: %. Ошибок нет.
pub fn expected_of(
    cex: bool,
    native: bool,
    measured: Option<Decimal>,
    token_tax: Option<Decimal>,
) -> Decimal {
    if !cex || native {
        return Decimal::ZERO;
    }
    measured
        .or(token_tax)
        .unwrap_or_default()
        .max(Decimal::ZERO)
}

/// Ожидаемое удержание биржи `to` при зачислении монеты, %. Ошибок нет.
pub fn expected_pct(core: &Core, to: &VenueId, a: &Asset) -> Decimal {
    let cex = super::trade_ops::kind(core, to) == Ok(VenueKind::Cex);
    if !cex || super::native::is_native(core, a) {
        return Decimal::ZERO;
    }
    let measured = super::deposit_hold::get(to, a).map(|h| h.pct);
    let tax = if measured.is_some() {
        None
    } else {
        token_tax(core, a)
    };
    expected_of(cex, false, measured, tax)
}

/// Налог перевода токена по проверкам DEX, %. None — не измерен или нет адреса. Ошибок нет.
fn token_tax(core: &Core, a: &Asset) -> Option<Decimal> {
    let t = a.bsc_contract?;
    let kinds = rd(&core.inner.kinds);
    super::transfer_tax::measured(&rd(&core.inner.market), &kinds, t)
}

/// Что делает: строка затрат «удержание биржи при зачислении» на приход. Принимает: процент, приход в монете (вывод
/// минус затраты вывода в монете), тикер. Возвращает: затрата DepositHold в монете (None — 0 %). Ошибок нет.
pub fn line(pct: Decimal, arrive: Decimal, ticker: &str) -> Option<Cost> {
    (pct > Decimal::ZERO && arrive > Decimal::ZERO).then(|| {
        Cost::new(
            CostKind::DepositHold,
            arrive * pct / Decimal::ONE_HUNDRED,
            ticker.to_string(),
            false,
        )
    })
}

/// Что делает: добавляет к затратам вывода строку удержания получателя `to` (приход — сумма минус затраты в монете).
/// Принимает: ядро, получателя, монету, сумму вывода, затраты вывода. Возвращает: затраты. Ошибок нет.
pub fn add(core: &Core, to: &VenueId, a: &Asset, amount: Decimal, mut c: Vec<Cost>) -> Vec<Cost> {
    let in_coin: Decimal = c
        .iter()
        .filter(|x| !x.in_price && super::convert::in_coin(x, &a.ticker))
        .map(|x| x.amount)
        .sum();
    if let Some(l) = line(expected_pct(core, to, a), amount - in_coin, &a.ticker) {
        c.push(l);
    }
    c
}
