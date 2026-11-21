//! Файл: venues/gate/all_balances_parse.rs
//! Что делает: чистый разбор ответов Gate о счетах (Value → строки балансов и пояснение) для панели «Балансы»
//! (решение 144): изолированная маржа, фьючерсы и фьючерсы с поставкой, опционы, Earn (Uni Lend), итоги по счетам.
//! Только ненулевые строки. Без сети — тестируется на записанных ответах.
//! Кто вызывает: all_balances.rs.
//! От чего зависит: num.rs, errors.rs, core/types.
//! Настройки: нет.
//!
//! Поля — по официальной документации https://www.gate.com/docs/developers/apiv4/en/ и моделям официального SDK
//! (https://github.com/gate/gateapi-python/tree/master/docs):
//! - MarginAccount (GET /margin/accounts): currency_pair, base и quote (MarginAccountCurrency: currency, available
//!   «available = margin + borrowed», locked «Frozen funds», borrowed, interest «Unpaid interest»).
//! - FuturesAccount / DeliveryAccount (GET /futures/{settle}/accounts, GET /delivery/{settle}/accounts): currency, total
//!   («Balance, only applicable to classic contract account»), available, unrealised_pnl, order_margin.
//! - OptionsAccount (GET /options/accounts): currency, total, available, equity («balance + option position value»),
//!   unrealised_pnl.
//! - UniLend (GET /earn/uni/lends): currency, amount «Total Lending Amount», frozen_amount «Pending Redemption Amount»,
//!   lent_amount «Lent Amount».
//! - TotalBalance (GET /wallet/total_balance): total и details — словарь счетов (AccountBalance: amount, currency,
//!   unrealised_pnl, borrowed); имена ключей details — из описания модели (cross_margin, spot, finance, margin, quant,
//!   futures, delivery, warrant, cbbc, meme_box, options, payment).

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Balance, VenueId};
use crate::gateway::errors::GatewayResult;

use super::{errors, num};

/// Разобранный счёт: ненулевые строки и пояснение (None — нечего пояснить).
pub struct Part {
    pub balances: Vec<Balance>,
    pub note: Option<String>,
}

/// Строка баланса; None — обе суммы нулевые. Ошибок нет.
fn row(venue: &VenueId, asset: String, free: Decimal, locked: Decimal) -> Option<Balance> {
    (!free.is_zero() || !locked.is_zero()).then(|| Balance {
        venue: venue.clone(),
        asset,
        free,
        locked,
    })
}

/// Число поля или 0, если поля нет (поля моделей SDK помечены [optional]). Ошибка BadResponse — не число.
fn or_zero(v: &Value, field: &str) -> GatewayResult<Decimal> {
    Ok(num::dec_opt(v, field)?.unwrap_or_default())
}

/// Что делает: изолированная маржа — по строке на монету пары («USDT · BTC_USDT»): free = available, locked = locked.
/// Принимает: ответ /margin/accounts (массив). Возвращает: строки и пояснение о займах и процентах.
/// Ошибки: BadResponse — не массив или поле не число.
pub fn margin(v: &Value, venue: &VenueId) -> GatewayResult<Part> {
    let arr = v
        .as_array()
        .ok_or_else(|| errors::bad("/margin/accounts не массив", &v.to_string()))?;
    let (mut balances, mut debts) = (Vec::new(), Vec::new());
    for acc in arr {
        let pair = num::text(acc, "currency_pair")?;
        for side in ["base", "quote"] {
            let Some(c) = acc.get(side) else { continue };
            let cur = num::text(c, "currency")?;
            let (borrowed, interest) = (or_zero(c, "borrowed")?, or_zero(c, "interest")?);
            if !borrowed.is_zero() || !interest.is_zero() {
                debts.push(format!(
                    "{cur} · {pair}: заём {borrowed}, проценты {interest}"
                ));
            }
            let (free, locked) = (or_zero(c, "available")?, or_zero(c, "locked")?);
            balances.extend(row(venue, format!("{cur} · {pair}"), free, locked));
        }
    }
    let note = (!debts.is_empty()).then(|| {
        format!(
            "available включает заём (margin + borrowed); {}",
            debts.join("; ")
        )
    });
    Ok(Part { balances, note })
}

/// Что делает: фьючерсный счёт (бессрочные и с поставкой — одна модель): free = available,
/// locked = total − available (не меньше 0; total — баланс классического счёта).
/// Принимает: объект ответа. Возвращает: строку и пояснение (баланс, нереализованный PnL, маржа ордеров).
/// Ошибки: BadResponse — не объект, нет currency, поле не число.
pub fn contract(v: &Value, venue: &VenueId) -> GatewayResult<Part> {
    let cur = num::text(v, "currency")?;
    let (total, avail) = (or_zero(v, "total")?, or_zero(v, "available")?);
    let (pnl, om) = (or_zero(v, "unrealised_pnl")?, or_zero(v, "order_margin")?);
    // Живой ответ 04.11: total ≈ 0, available = 49.99 = USDT спота — available Gate считает с учётом других счетов
    // (кросс-маржа), это не деньги фьючерсного счёта. Деньги счёта — total; доступно — не больше total.
    let (free, locked) = own(total, avail);
    let balances = row(venue, cur.clone(), free, locked).into_iter().collect();
    let mut note = (!total.is_zero() || !pnl.is_zero() || !om.is_zero())
        .then(|| format!("баланс {total} {cur}, нереализованный PnL {pnl}, маржа ордеров {om}"));
    cross(&mut note, total, avail, &cur);
    Ok(Part { balances, note })
}

/// Что делает: опционный счёт: деньги счёта — total (свободно — available, но не больше total; own()).
/// Возвращает: строку и пояснение (эквити, нереализованный PnL). Ошибки: BadResponse.
pub fn options(v: &Value, venue: &VenueId) -> GatewayResult<Part> {
    let cur = num::text(v, "currency")?;
    let (total, avail) = (or_zero(v, "total")?, or_zero(v, "available")?);
    let (equity, pnl) = (or_zero(v, "equity")?, or_zero(v, "unrealised_pnl")?);
    let (free, locked) = own(total, avail);
    let balances = row(venue, cur.clone(), free, locked).into_iter().collect();
    let mut note = (!equity.is_zero() || !pnl.is_zero())
        .then(|| format!("эквити {equity} {cur}, нереализованный PnL {pnl}"));
    cross(&mut note, total, avail, &cur);
    Ok(Part { balances, note })
}

/// Деньги самого счёта: (свободно, заморожено) из total и available — свободно не больше total (available Gate может
/// включать средства других счетов), заморожено = total − свободно. Ошибок нет.
fn own(total: Decimal, avail: Decimal) -> (Decimal, Decimal) {
    let free = avail.min(total).max(Decimal::ZERO);
    (free, (total - free).max(Decimal::ZERO))
}

/// Пояснение «доступно с учётом других счетов», если available больше денег счёта. Ошибок нет.
fn cross(note: &mut Option<String>, total: Decimal, avail: Decimal, cur: &str) {
    if avail > total {
        let t = format!(
            "доступно для сделок с учётом других счетов {avail} {cur} (это не деньги этого счёта)"
        );
        *note = Some(match note.take() {
            Some(n) => format!("{n}; {t}"),
            None => t,
        });
    }
}

/// Что делает: Earn (Uni Lend): по строке на монету — free = amount − frozen_amount (вложено), locked = frozen_amount
/// (ждёт погашения). Принимает: записи всех страниц /earn/uni/lends. Возвращает: строки и пояснение (сколько выдано
/// в заём — lent_amount). Ошибки: BadResponse.
pub fn uni_lends(items: &[Value], venue: &VenueId) -> GatewayResult<Part> {
    let (mut balances, mut lent) = (Vec::new(), Vec::new());
    for it in items {
        let cur = num::text(it, "currency")?;
        let (amount, frozen) = (or_zero(it, "amount")?, or_zero(it, "frozen_amount")?);
        let l = or_zero(it, "lent_amount")?;
        if !l.is_zero() {
            lent.push(format!("{cur} {l}"));
        }
        balances.extend(row(venue, cur, amount - frozen, frozen));
    }
    let note = (!lent.is_empty()).then(|| format!("выдано в заём: {}", lent.join(", ")));
    Ok(Part { balances, note })
}

/// Русское имя счёта для ключа details модели TotalBalance (список — из описания модели в документации; новый ключ —
/// как есть). Ошибок нет.
fn total_name(key: &str) -> &str {
    match key {
        "cross_margin" => "кросс-маржа",
        "spot" => "спот",
        "finance" => "Finance (Earn)",
        "margin" => "маржа",
        "quant" => "Quant",
        "futures" => "фьючерсы",
        "delivery" => "фьючерсы с поставкой",
        "warrant" => "Warrant",
        "cbbc" => "CBBC",
        "meme_box" => "Alpha",
        "options" => "опционы",
        "payment" => "Payment",
        other => other,
    }
}

/// Что делает: итоги /wallet/total_balance одной строкой — всего и ненулевые счета (оценка Gate, кэш до 1 мин).
/// Принимает: ответ. Возвращает: текст пояснения. Ошибки: BadResponse — нет total или поле не число.
pub fn totals(v: &Value) -> GatewayResult<String> {
    let t = v
        .get("total")
        .ok_or_else(|| errors::bad("/wallet/total_balance без total", &v.to_string()))?;
    let cur = num::text_opt(t, "currency").unwrap_or_default();
    let mut parts = vec![format!("всего {} {cur}", or_zero(t, "amount")?)];
    if let Some(d) = v.get("details").and_then(Value::as_object) {
        for (k, a) in d {
            let amount = or_zero(a, "amount")?;
            let (pnl, debt) = (or_zero(a, "unrealised_pnl")?, or_zero(a, "borrowed")?);
            if amount.is_zero() && pnl.is_zero() && debt.is_zero() {
                continue;
            }
            let mut s = format!("{} {amount}", total_name(k));
            if !pnl.is_zero() {
                s.push_str(&format!(" (PnL {pnl})"));
            }
            if !debt.is_zero() {
                s.push_str(&format!(" (заём {debt})"));
            }
            parts.push(s);
        }
    }
    Ok(format!("Оценка Gate (кэш до 1 мин): {}", parts.join("; ")))
}
