//! Файл: venues/bingx/tap_diff.rs
//! Что делает: разбор кадра «<SYMBOL>@incrDepth» BingX для окна (дополнение решения 125): полный стакан (action «all»)
//! или изменения (action «update») с lastUpdateId → TapView::Diff ядра; номер для проверки разрыва (feed.rs).
//! Кто вызывает: tap.rs (decode), feed.rs (of_channel — кадр своего канала, gap — разрыв lastUpdateId), тесты.
//! От чего зависит: num.rs, core/types.
//! Настройки: нет.
//!
//! Источники расходятся (оба официальные), поэтому разбор мягкий — принимаются оба вида уровней:
//! - docs-v3, Spot → «Incremental and Full Depth Information», пример dataUpdateDemo: asks/bids — ОБЪЕКТ
//!   {"цена": "количество"}, поля action, lastUpdateId, sourceUpdateId; время — timestamp верхнего уровня;
//! - api-ai-skills, spot-ws-market/api-reference.md: data.bids/asks — МАССИВ [[цена, количество], …], время — data.T.
//!
//! Количество 0 — уровня больше нет (docs-v3: «If the quantity corresponding to the price is 0, the price level should
//! be removed»). Какой вид приходит на деле — проверить на первой живой записи (tests/README.md).

use serde_json::Value;

use crate::core::types::{PriceLevel, TapDecoded, TapView};

use super::num;

/// Что делает: action «all» ли и lastUpdateId из data кадра. Возвращает: (полный ли, номер). Ошибок нет.
pub fn update_id(data: &Value) -> (bool, Option<i64>) {
    let full = data.get("action").and_then(Value::as_str) == Some("all");
    let id = match data.get("lastUpdateId") {
        Some(Value::String(s)) => s.parse().ok(),
        Some(v) => v.as_i64(),
        None => None,
    };
    (full, id)
}

/// Что делает: разбирает кадр @incrDepth. Принимает: текст. Возвращает: TapDecoded (Diff; не тот кадр — Other).
/// Ошибки: текст причины (не JSON, уровень не разобран).
pub fn decode(text: &str) -> Result<TapDecoded, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("не JSON: {e}"))?;
    let Some(data) = v.get("data").filter(|d| d.is_object()) else {
        return Ok(TapDecoded {
            view: TapView::Other(text.to_string()),
            exchange_ms: None,
        });
    };
    let (full, id) = update_id(data);
    let exchange_ms = v
        .get("timestamp")
        .and_then(Value::as_i64)
        .or_else(|| data.get("T").and_then(Value::as_i64));
    Ok(TapDecoded {
        view: TapView::Diff {
            bids: levels(data.get("bids"), "bids")?,
            asks: levels(data.get("asks"), "asks")?,
            full,
            from: id,
            to: id,
        },
        exchange_ms,
    })
}

/// Уровни объектом {цена: количество} или массивом [[цена, количество]]. Нет поля — пусто. Ошибка — текст причины.
fn levels(v: Option<&Value>, side: &str) -> Result<Vec<PriceLevel>, String> {
    let dec =
        |x: &Value, what: &str| num::to_dec(x).ok_or(format!("{side}: {what} «{x}» не число"));
    match v {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(p, q)| {
                Ok(PriceLevel {
                    price: dec(&Value::String(p.clone()), "цена")?,
                    qty: dec(q, "количество")?,
                })
            })
            .collect(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|l| {
                let pair = l
                    .as_array()
                    .filter(|p| p.len() >= 2)
                    .ok_or(format!("{side}: уровень не [цена, количество]"))?;
                Ok(PriceLevel {
                    price: dec(&pair[0], "цена")?,
                    qty: dec(&pair[1], "количество")?,
                })
            })
            .collect(),
        Some(o) => Err(format!("{side}: не объект и не массив: {o}")),
    }
}

/// Кадр канала: dataType совпадает с подпиской и есть data. Возвращает: разобранный JSON. Ошибок нет.
pub fn of_channel(text: &str, ch: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    (v.get("dataType").and_then(|d| d.as_str()) == Some(ch)
        && v.get("data").is_some_and(|d| !d.is_null()))
    .then_some(v)
}

/// Разрыв lastUpdateId (см. шапку): action «all» начинает отсчёт заново. Возвращает: причину разрыва. Ошибок нет.
pub fn gap(v: &serde_json::Value, last: &mut Option<i64>) -> Option<String> {
    let (full, id) = update_id(&v["data"]);
    let id = id?;
    let prev = last.replace(id);
    match prev {
        Some(p) if !full && id != p + 1 => Some(format!(
            "разрыв lastUpdateId: после {p} пришёл {id} — нужен новый полный стакан"
        )),
        _ => None,
    }
}
