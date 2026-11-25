//! Snapshot known when a spot withdrawal was attempted. Spot free is not a withdrawable balance.
use super::ctx::{Ctx, SECRET_API_KEY};
use chrono::Utc;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

#[derive(Clone)]
struct Observation {
    free: Decimal,
    locked: Option<Decimal>,
    at: i64,
    origin: &'static str,
}
type Book = HashMap<(u64, String), Observation>;
static LAST: LazyLock<Mutex<Book>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn mark(ctx: &Ctx) -> Option<u64> {
    ctx.vault.mark(SECRET_API_KEY)
}

pub(super) fn record(
    mark: Option<u64>,
    ticker: &str,
    free: Decimal,
    locked: Option<Decimal>,
    at: i64,
    origin: &'static str,
) {
    let Some(mark) = mark else { return };
    if at <= 0 {
        return;
    }
    let mut map = LAST.lock().unwrap_or_else(|p| p.into_inner());
    let key = (mark, ticker.to_owned());
    if map.get(&key).is_none_or(|v| v.at <= at) {
        map.insert(
            key,
            Observation {
                free,
                locked,
                at,
                origin,
            },
        );
    }
}

pub(super) fn describe(mark: Option<u64>, wallet: &str, ticker: &str) -> String {
    if wallet != "0" {
        return "снимок funding на момент запроса отсутствует".into();
    }
    let observation = mark.and_then(|m| {
        LAST.lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&(m, ticker.to_owned()))
            .cloned()
    });
    match observation {
        None => "нет снимка spot".into(),
        Some(o) => format!(
            "снимок spot {} t={} (возраст {} мс), свободно {}, заблокировано {}",
            o.origin,
            o.at,
            Utc::now().timestamp_millis() - o.at,
            o.free,
            o.locked
                .map_or_else(|| "не сообщено".into(), |n| n.to_string())
        ),
    }
}
