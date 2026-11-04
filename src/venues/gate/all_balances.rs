//! Файл: venues/gate/all_balances.rs
//! Что делает: панель «Балансы» (решение 144) — балансы всех счетов Gate, только чтение: спот, изолированная маржа,
//! фьючерсы по каждой валюте расчёта, фьючерсы с поставкой, опционы, Earn (Uni Lend) и итог по всем счетам (оценка
//! Gate). Запросы — параллельно; ошибка одного счёта идёт в его error и не мешает остальным.
//! Кто вызывает: mod.rs (Venue::account_balances), панель — примерно раз в 3 с, пока открыта.
//! От чего зависит: rest.rs (подпись, пауза после 429), account.rs (спот), all_balances_parse.rs (разбор), ctx.rs.
//! Настройки [gate]: venue.futures_settles — валюты расчёта бессрочных фьючерсов (список), venue.delivery_settles —
//! валюты расчёта фьючерсов с поставкой (список). Не задана — в error счёта; пустой список — счёт не читается.
//!
//! Эндпоинты — официальная документация https://www.gate.com/docs/developers/apiv4/en/ (пути — таблица методов
//! официального SDK https://github.com/gate/gateapi-python/blob/master/README.md):
//! - Spot → «List spot trading accounts»: GET /spot/accounts (account.rs).
//! - Margin → «Margin account list»: GET /margin/accounts (изолированная маржа классического счёта).
//! - Futures → «Get futures account»: GET /futures/{settle}/accounts («GET /futures/btc/accounts returns accounts in BTC»;
//!   settle — usdt, btc, с v4.106.126 и usd1).
//! - Delivery → «Get futures account»: GET /delivery/{settle}/accounts (settle в примере SDK — usdt).
//! - Options → «Query account information»: GET /options/accounts.
//! - Earn Uni → «Query user's lending order list»: GET /earn/uni/lends?page=&limit= (limit — максимум 100 по документации).
//! - Wallet → «Query personal account totals»: GET /wallet/total_balance (оценка всех счетов в USDT по умолчанию,
//!   «may be cached for up to 1 minute» — только для пояснения, не для расчётов).
//!
//! Все — тот же адрес REST (venue.rest_url = https://api.gateio.ws/api/v4); отдельный домен фьючерсов fx-api.gateio.ws в
//! документации помечен устаревшим — не нужен.
//!
//! Лимиты («Frequency limit rule»; счёт идёт по UID и по каждой конечной точке отдельно): за один вызов панели —
//! по 1 запросу на точку (фьючерсы — по 1 на settle, Earn — по 1 на страницу из 100 записей). Раз в 3 с — ≈3,3 запроса
//! за 10 с на точку: /spot/accounts — лимит 200/10 с («Spot: all other endpoints»; общий с торговлей: бот берёт 1,7 %),
//! /futures/{settle}/accounts — 200/10 с (даже если usdt+btc+usd1 считаются одной точкой — 10/10 с = 5 %),
//! /delivery/…, /options/accounts — по 200/10 с, /wallet/total_balance — 80/10 с (4 %), /earn/uni/lends — «Other
//! private: Earn» 150/10 с (2,2 %), /margin/accounts — отдельной строки в таблице нет, минимальный лимит таблицы для
//! приватных чтений — 80/10 с (4 %). Ордера и отмены считаются по своим точкам (10/с и 200/с) — панель их не трогает.

use futures_util::future::join_all;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{AccountBalances, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::all_balances_parse::{self as parse, Part};
use super::ctx::Ctx;
use super::errors::Op;
use super::{account, rest};

/// Разбор ответа счёта (all_balances_parse.rs).
type Parser = fn(&Value, &VenueId) -> GatewayResult<Part>;

/// Что делает: все счета Gate. Возвращает: счета (у каждого — строки или error).
/// Ошибки: только если не прочитан ни один счёт — ошибка спота.
pub async fn account_balances(ctx: &Ctx) -> GatewayResult<Vec<AccountBalances>> {
    let fut = settles(ctx, "venue.futures_settles");
    let del = settles(ctx, "venue.delivery_settles");
    let (spot, margin, options, earn, total, futs, dels) = tokio::join!(
        account::balances(ctx, None),
        read(
            ctx,
            "Маржа (изолированная)".into(),
            "/margin/accounts".into(),
            "Margin (perms: margin)",
            parse::margin
        ),
        read(
            ctx,
            "Опционы".into(),
            "/options/accounts".into(),
            "Options (perms: options)",
            parse::options
        ),
        earn(ctx),
        total(ctx),
        group(ctx, &fut, "Фьючерсы", "futures", "Futures (perms: futures)"),
        group(
            ctx,
            &del,
            "Фьючерсы с поставкой",
            "delivery",
            "Delivery (perms: delivery)"
        ),
    );
    let spot_err = spot.as_ref().err().cloned();
    let mut out = vec![match spot {
        Ok(b) => ok(
            "Спот".into(),
            b.into_iter()
                .filter(|x| !x.free.is_zero() || !x.locked.is_zero())
                .collect(),
            None,
        ),
        Err(e) => failed("Спот".into(), reason(&e, "Spot (perms: spot)")),
    }];
    out.extend([margin]);
    out.extend(futs);
    out.extend(dels);
    out.extend([options, earn, total]);
    match spot_err {
        Some(e) if out.iter().all(|a| a.error.is_some()) => Err(e),
        _ => Ok(out),
    }
}

/// Что делает: валюты расчёта из настройки-списка; значения — только буквы и цифры (идут в путь URL).
/// Возвращает: список или текст причины для error счёта. Ошибок нет.
fn settles(ctx: &Ctx, name: &str) -> Result<Vec<String>, String> {
    let list = ctx
        .settings
        .list(name, ctx.at())
        .map_err(|_| format!("не задана {name} — счёт не читается"))?;
    match list
        .iter()
        .find(|s| s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        Some(bad) => Err(format!(
            "{name}: «{bad}» — не валюта расчёта (только буквы и цифры)"
        )),
        None => Ok(list.into_iter().map(|s| s.to_lowercase()).collect()),
    }
}

/// Что делает: счета одного вида по всем валютам расчёта («Фьючерсы USDT»…) параллельно.
/// Принимает: список или причину, имя для человека, раздел пути (futures / delivery), право ключа.
/// Возвращает: счета; настройка не задана — один счёт с error. Ошибок нет.
async fn group(
    ctx: &Ctx,
    list: &Result<Vec<String>, String>,
    title: &str,
    section: &str,
    perm: &str,
) -> Vec<AccountBalances> {
    match list {
        Err(m) => vec![failed(title.into(), m.clone())],
        Ok(l) => {
            join_all(l.iter().map(|s| {
                let name = format!("{title} {}", s.to_uppercase());
                read(
                    ctx,
                    name,
                    format!("/{section}/{s}/accounts"),
                    perm,
                    parse::contract,
                )
            }))
            .await
        }
    }
}

/// Что делает: один счёт одним подписанным GET. Принимает: имя, путь, право ключа (для текста), разбор.
/// Возвращает: счёт со строками или с error. Ошибок нет.
async fn read(ctx: &Ctx, name: String, path: String, perm: &str, f: Parser) -> AccountBalances {
    match rest::signed(ctx, Method::GET, &path, vec![], None, Op::Other)
        .await
        .and_then(|v| f(&v, &ctx.venue()))
    {
        Ok(p) => ok(name, p.balances, p.note),
        Err(e) => failed(name, reason(&e, perm)),
    }
}

/// Что делает: Earn (Uni Lend) — все страницы /earn/uni/lends (limit 100 — максимум по документации; страница короче —
/// последняя). Возвращает: счёт «Earn (Uni Lend)». Ошибок нет (ошибка — в error).
async fn earn(ctx: &Ctx) -> AccountBalances {
    let name = "Earn (Uni Lend)".to_string();
    // Максимум limit — из документации метода («Default: 100, minimum: 1, maximum: 100»).
    const PAGE: usize = 100;
    let mut items = Vec::new();
    for page in 1.. {
        let p = vec![("page", page.to_string()), ("limit", PAGE.to_string())];
        let arr = match rest::signed(ctx, Method::GET, "/earn/uni/lends", p, None, Op::Other).await
        {
            Ok(Value::Array(a)) => a,
            Ok(v) => {
                return failed(
                    name,
                    format!("Gate: непонятный ответ: /earn/uni/lends не массив ({v})"),
                );
            }
            Err(e) => return failed(name, reason(&e, "Earn")),
        };
        let last = arr.len() < PAGE;
        items.extend(arr);
        if last {
            break;
        }
    }
    match parse::uni_lends(&items, &ctx.venue()) {
        Ok(p) => ok(name, p.balances, p.note),
        Err(e) => failed(name, e.message),
    }
}

/// Что делает: итог по всем счетам (/wallet/total_balance) — строк нет, только пояснение. Ошибок нет.
async fn total(ctx: &Ctx) -> AccountBalances {
    let name = "Итого по счетам (оценка Gate)".to_string();
    match rest::signed(
        ctx,
        Method::GET,
        "/wallet/total_balance",
        vec![],
        None,
        Op::Other,
    )
    .await
    .and_then(|v| parse::totals(&v))
    {
        Ok(n) => ok(name, vec![], Some(n)),
        Err(e) => failed(name, reason(&e, "Wallet (perms: wallet)")),
    }
}

/// Текст причины: нет права у ключа — с именем права, иначе — текст ошибки как есть. Ошибок нет.
fn reason(e: &GatewayError, perm: &str) -> String {
    if e.code == ErrorCode::InsufficientPermissions {
        format!("нет права {perm} у ключа — {}", e.message)
    } else {
        e.message.clone()
    }
}

/// Прочитанный счёт.
fn ok(
    account: String,
    balances: Vec<crate::core::types::Balance>,
    note: Option<String>,
) -> AccountBalances {
    AccountBalances {
        account,
        balances,
        error: None,
        note,
    }
}

/// Непрочитанный счёт с причиной.
fn failed(account: String, error: String) -> AccountBalances {
    AccountBalances {
        account,
        balances: vec![],
        error: Some(error),
        note: None,
    }
}
