//! Файл: core/usdt_sent_backfill.rs
//! Что делает: решение 236, дополнение п. 5 (техлид 09.11 по поручению Тиграна) — разовая запись «ушло» (sent) в СТАРЫЕ
//! части возврата USDT «вернулись» (UsdtReturn::Done без sent — записи до решения 236). Затраты такой части бот считает
//! как запрошено − зачислено (deal_return_cost::part_cost) — вместе с остатком округления вывода, который лежит на
//! площадке-отправителе (MEXC: точность вывода USDT 0.1; 145 возвратов, 6.34 USDT — docs/notes/deal-accounting-2026-11-09.md).
//! Факт «ушло» есть только в журнале: запись core.transfer_sent, которую пишет core/transfers.rs после хеша вывода, —
//! «<id> <ушло> <монета>: <откуда> → <куда>, хеш <tx>»; <ушло> — сумма вывода после округления до точности вывода
//! (transfers.rs floored_on); <id> перегона USDT — id сделки + «u» (первая попытка, usdt_return_job.rs), «u<n>» (повтор,
//! usdt_return_retry.rs) или «m<цифры>» («Перегнать», manual_usdt.rs).
//! Сопоставление части и записи журнала: id записи = id сделки + такой хвост, откуда и куда — как у части; ровно одна
//! такая запись (иначе — пропуск с причиной); зачислено ≤ ушло ≤ запрошено (иначе — пропуск). В запись сделки
//! дописывается только поле sent этой части (JSON записи правится точечно: остальные поля не трогаются). Итог сделки бот
//! пересчитает при загрузке (ledger_load.rs → deal_return_cost::apply: разница с уже учтённым return_cost_usd); итоги
//! прошлых дней не пересчитываются (как решение 236).
//! Кто вызывает: src/bin/usdt_sent_backfill.rs (программа; бот остановлен для записи); тесты.
//! От чего зависит: storage/db.rs (core_state, journal), serde_json, rust_decimal.
//! Настройки: нет.

use std::str::FromStr;

use rusqlite::params;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::storage::db::{Db, DbError};

/// Вид записи журнала с фактом вывода (core/transfers.rs).
pub const SENT_KIND: &str = "core.transfer_sent";

/// Запись журнала о выводе, разобранная.
#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    /// Первое слово записи: id перевода (у перегона USDT — id сделки + хвост).
    pub id: String,
    /// Ушло с площадки (после округления до точности вывода).
    pub qty: Decimal,
    pub ticker: String,
    pub from: String,
    pub to: String,
    /// Время записи журнала (RFC 3339).
    pub at: String,
}

/// Часть возврата, которой дописывается sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    pub deal: String,
    /// 0 — usdt_return (первая часть), n — usdt_return_parts[n − 1].
    pub slot: usize,
    pub from: String,
    pub to: String,
    pub amount: Decimal,
    pub credited: Decimal,
    pub sent: Decimal,
    /// Время записи журнала, из которой взято sent.
    pub at: String,
}

/// Часть без sent, которую не исправить, — с причиной.
#[derive(Debug, Clone, PartialEq)]
pub struct Skip {
    pub deal: String,
    pub from: String,
    pub to: String,
    pub amount: Decimal,
    pub why: String,
}

/// Что делает: разбирает текст записи core.transfer_sent «<id> <ушло> <монета>: <откуда> → <куда>, хеш <tx>».
/// Принимает: текст, время записи. Возвращает: Some — разобрано; None — другой вид текста. Ошибок нет.
pub fn parse(message: &str, at: &str) -> Option<Sent> {
    let (head, tail) = message.split_once(": ")?;
    let mut h = head.split(' ');
    let (id, qty, ticker) = (h.next()?, h.next()?, h.next()?);
    if h.next().is_some() {
        return None;
    }
    let (route, _) = tail.split_once(", хеш ")?;
    let (from, to) = route.split_once(" → ")?;
    Some(Sent {
        id: id.to_string(),
        qty: Decimal::from_str(qty).ok()?,
        ticker: ticker.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        at: at.to_string(),
    })
}

/// Что делает: id записи — перегон USDT этой сделки: id сделки + «u», «u<цифры>» или «m<цифры>» (см. шапку).
/// Ошибок нет.
pub fn is_return_of(id: &str, deal: &str) -> bool {
    let Some(tail) = id.strip_prefix(deal) else {
        return false;
    };
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    match tail.split_at_checked(1) {
        Some(("u", rest)) => digits(rest),
        Some(("m", rest)) => !rest.is_empty() && digits(rest),
        _ => false,
    }
}

/// Что делает: части сделки «вернулись» без sent (JSON записи сделки). Принимает: JSON. Возвращает: (слот, часть).
/// Ошибок нет.
fn old_done(body: &Value) -> Vec<(usize, &Value)> {
    let first = body.get("usdt_return").map(|p| (0, p));
    let rest = body
        .get("usdt_return_parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, p)| (i + 1, p));
    first
        .into_iter()
        .chain(rest)
        .filter(|(_, p)| p.get("state").and_then(Value::as_str) == Some("done"))
        .filter(|(_, p)| p.get("sent").is_none_or(Value::is_null))
        .collect()
}

/// Число из поля JSON части (Decimal пишется строкой). Ошибок нет.
fn num(p: &Value, k: &str) -> Option<Decimal> {
    match p.get(k)? {
        Value::String(s) => Decimal::from_str(s).ok(),
        Value::Number(n) => Decimal::from_str(&n.to_string()).ok(),
        _ => None,
    }
}

/// Строка из поля JSON части. Ошибок нет.
fn text(p: &Value, k: &str) -> String {
    p.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Что делает: что исправить и что пропустить. Принимает: сделки (id, JSON), записи журнала, площадки-отправители
/// (пусто — все). Возвращает: (исправления, пропуски). Ошибок нет.
pub fn plan(
    deals: &[(String, Value)],
    sents: &[Sent],
    only_from: &[String],
) -> (Vec<Fix>, Vec<Skip>) {
    let (mut fixes, mut skips) = (vec![], vec![]);
    for (deal, body) in deals {
        for (slot, p) in old_done(body) {
            let (from, to) = (text(p, "from"), text(p, "to"));
            if !only_from.is_empty() && !only_from.contains(&from) {
                continue;
            }
            let (Some(amount), Some(credited)) = (num(p, "amount"), num(p, "credited")) else {
                skips.push(skip(
                    deal,
                    &from,
                    &to,
                    Decimal::ZERO,
                    "в части нет amount / credited",
                ));
                continue;
            };
            let hits: Vec<&Sent> = sents
                .iter()
                .filter(|s| is_return_of(&s.id, deal) && s.from == from && s.to == to)
                .collect();
            let why = match hits.as_slice() {
                [] => Some("в журнале нет записи core.transfer_sent этого перегона".to_string()),
                [s] if s.qty > amount => {
                    Some(format!("ушло {} больше запрошенного {amount}", s.qty))
                }
                [s] if s.qty < credited => {
                    Some(format!("ушло {} меньше зачисленного {credited}", s.qty))
                }
                [_] => None,
                many => Some(format!(
                    "в журнале {} записей этого перегона — неясно, какая",
                    many.len()
                )),
            };
            match (why, hits.first()) {
                (None, Some(s)) => fixes.push(Fix {
                    deal: deal.clone(),
                    slot,
                    from,
                    to,
                    amount,
                    credited,
                    sent: s.qty,
                    at: s.at.clone(),
                }),
                (w, _) => skips.push(skip(deal, &from, &to, amount, &w.unwrap_or_default())),
            }
        }
    }
    (fixes, skips)
}

/// Пропуск. Ошибок нет.
fn skip(deal: &str, from: &str, to: &str, amount: Decimal, why: &str) -> Skip {
    Skip {
        deal: deal.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        amount,
        why: why.to_string(),
    }
}

/// Что делает: дописывает sent в часть JSON записи сделки (только это поле). Принимает: JSON, исправление.
/// Возвращает: true — дописано; false — части нет или она уже не «вернулись» без sent. Ошибок нет.
pub fn apply(body: &mut Value, f: &Fix) -> bool {
    let part = if f.slot == 0 {
        body.get_mut("usdt_return")
    } else {
        body.get_mut("usdt_return_parts")
            .and_then(|v| v.get_mut(f.slot - 1))
    };
    let Some(Value::Object(m)) = part else {
        return false;
    };
    let fresh = m.get("state").and_then(Value::as_str) == Some("done")
        && m.get("sent").is_none_or(Value::is_null);
    if fresh {
        m.insert("sent".into(), Value::String(f.sent.to_string()));
    }
    fresh
}

/// Что делает: сделки (id, JSON) и записи журнала core.transfer_sent из базы бота. Принимает: базу.
/// Возвращает: (сделки, разобранные записи). Ошибки: база, JSON сделки.
pub async fn load(db: &Db) -> Result<(Vec<(String, Value)>, Vec<Sent>), DbError> {
    let rows: Vec<(String, String)> = db
        .call(|c| {
            let mut st = c.prepare("SELECT id, body FROM core_state WHERE kind = 'deal'")?;
            let it = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            it.collect()
        })
        .await?;
    let mut deals = Vec::with_capacity(rows.len());
    for (id, body) in rows {
        let v =
            serde_json::from_str(&body).map_err(|e| DbError::Json(format!("сделка {id}: {e}")))?;
        deals.push((id, v));
    }
    let lines: Vec<(String, String)> = db
        .call(|c| {
            let mut st =
                c.prepare("SELECT message, at FROM journal WHERE kind = ?1 ORDER BY id")?;
            let it = st.query_map(params![SENT_KIND], |r| Ok((r.get(0)?, r.get(1)?)))?;
            it.collect()
        })
        .await?;
    let sents = lines.iter().filter_map(|(m, at)| parse(m, at)).collect();
    Ok((deals, sents))
}

/// Что делает: записывает исправленные сделки одной транзакцией (body; updated_at не трогается — запись не новая по
/// смыслу). Принимает: базу, (id, новый JSON). Возвращает: сколько записей обновлено. Ошибки: база.
pub async fn write(db: &Db, rows: Vec<(String, String)>) -> Result<usize, DbError> {
    db.call(move |c| {
        let tx = c.transaction()?;
        let mut n = 0;
        for (id, body) in &rows {
            n += tx.execute(
                "UPDATE core_state SET body = ?1 WHERE kind = 'deal' AND id = ?2",
                params![body, id],
            )?;
        }
        tx.commit()?;
        Ok(n)
    })
    .await
}
