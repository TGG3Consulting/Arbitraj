//! Файл: venues/kucoin/incr.rs
//! Что делает: решение 423 — разбор data кадра изменений стакана /market/level2 (subject trade.l2update) в кадр
//! локального стакана (local_book.rs Update): символ, sequenceStart, sequenceEnd, time, записи changes.
//! Кто вызывает: ws_fast.rs, тесты.
//! От чего зависит: local_book.rs (Update, Change), serde_json.
//! Настройки: нет.
//!
//! Официально (https://www.kucoin.com/docs-new/3470068w0, Orderbook - Increment): тема «/market/level2:{symbol},{symbol}»,
//! data {changes {asks, bids — ["price", "size", "sequence"]}, sequenceEnd, sequenceStart, symbol, time («milliseconds»)}.
//! Живая запись (tests/data/cex_samples_2026_11_04/kucoin_ws.jsonl): sequenceStart, sequenceEnd, time — числа; цена,
//! количество и номер записи — строки.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use super::local_book::{Change, Update};

/// Ошибка разбора: символ (если прочитан) и причина.
pub type Bad = (Option<String>, String);

/// Что делает: разбирает data кадра изменений. Принимает: data. Возвращает: (символ KuCoin, кадр).
/// Ошибка — (символ, если есть; причина): нет поля, число не число, запись не [цена, количество, номер].
pub fn parse(d: &Value) -> Result<(String, Update), Bad> {
    let symbol = d.get("symbol").and_then(Value::as_str).map(str::to_string);
    let bad = |why: String| (symbol.clone(), why);
    let num = |f: &str| {
        d.get(f)
            .and_then(Value::as_i64)
            .ok_or_else(|| bad(format!("нет {f} или не целое")))
    };
    let (start, end, time) = (num("sequenceStart")?, num("sequenceEnd")?, num("time")?);
    let ch = d.get("changes").ok_or_else(|| bad("нет changes".into()))?;
    let bids = side(ch, "bids").map_err(&bad)?;
    let asks = side(ch, "asks").map_err(&bad)?;
    let sym = symbol.clone().ok_or_else(|| bad("нет symbol".into()))?;
    Ok((
        sym,
        Update {
            start,
            end,
            time,
            bids,
            asks,
        },
    ))
}

/// Записи одной стороны: ["цена", "количество", "номер"] строками. Ошибка — текст причины.
fn side(ch: &Value, name: &str) -> Result<Vec<Change>, String> {
    let arr = ch
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("нет changes.{name}"))?;
    let mut out = Vec::with_capacity(arr.len());
    for l in arr {
        let p = l
            .as_array()
            .filter(|p| p.len() >= 3)
            .ok_or_else(|| format!("{name}: запись не [цена, количество, номер]"))?;
        let s = |x: &Value| x.as_str().map(str::trim).unwrap_or_default().to_string();
        let dec = |x: &Value, what: &str| {
            Decimal::from_str(&s(x)).map_err(|_| format!("{name}: {what} {x} не число"))
        };
        out.push(Change {
            price: dec(&p[0], "цена")?,
            size: dec(&p[1], "количество")?,
            seq: s(&p[2])
                .parse()
                .map_err(|_| format!("{name}: номер {} не целое", p[2]))?,
        });
    }
    Ok(out)
}
