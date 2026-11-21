//! Файл: venues/kucoin/all_balances.rs
//! Что делает: балансы ВСЕХ счетов KuCoin для панели «Балансы» (решение 144) — только чтение: Спот (trade), Фонд
//! (Funding, main), Фьючерсы (основной счёт по монетам маржи), Маржа (кросс), Маржа (изолированная), Опционы, Earn.
//! Запросы — параллельно; сбой одного счёта — в его error, остальные показываются.
//! Кто вызывает: mod.rs (Venue::account_balances), панель «Балансы» ≈ раз в 3 с, пока открыта.
//! От чего зависит: rest.rs (подпись, пауза пула после 429000, send_at для домена фьючерсов), reply.rs (пулы),
//! all_balances_parse.rs (разбор), ctx.rs.
//! Настройки: venue.futures_rest_url, venue.futures_balance_currencies [kucoin]; как у rest.rs.
//!
//! Официально (спецификация github.com/Kucoin/kucoin-universal-sdk spec/rest/api, коммит c2f39bd; пул / вес / право):
//! - Get Account List - Spot GET /api/v1/accounts (Management / 5 / General) https://www.kucoin.com/docs-new/api-3470125
//!   — без type: счета main и trade одним запросом; type=option — «Option account» (перечень значений type).
//! - Get Account - Cross Margin GET /api/v3/margin/accounts (Spot / 15 / General) .../api-3470127 — queryType=ALL
//!   «consistent aggregate query with the web side» (и обычная, и HF маржа).
//! - Get Account - Isolated Margin GET /api/v3/isolated/accounts (Spot / 15 / General) .../api-3470128 — queryType=ALL,
//!   без symbol — все пары.
//! - Get sub-account List - Futures Balance (V2) GET /api/v1/account-overview-all (Futures / 6 / General)
//!   .../api-3470134 — домен фьючерсов (x-domain Futures), currency — монета маржи (по умолчанию XBT), основной счёт —
//!   accountName main. Выбран вместо Get Account - Futures /api/v1/account-overview (api-3470129): у того право Futures,
//!   а у ключа бота права Futures быть не должно (account.rs).
//! - Get Account Holding (Earn) GET /api/v1/earn/hold-assets (Earn / 5 / General) .../api-3470273 — pageSize 10…500.
//!
//! Лимиты (https://www.kucoin.com/docs-new/rate-limit-rule-classic, квоты за 30 с, VIP0): за один вызов панели —
//! Management 5 + 5 = 10, Spot 15 + 15 = 30, Futures 6 × N (N — монет в venue.futures_balance_currencies), Earn 5.
//! Раз в 3 с → за 30 с: Management 100 из 2000 (5 %), Spot 300 из 4000 (7,5 %), Futures 60·N из 2000 (N = 3 → 9 %;
//! торговля бота этот пул не использует), Earn 50 из 2000 (2,5 %; торговля не использует).

use reqwest::Method;
use serde_json::Value;

use crate::core::types::AccountBalances;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::all_balances_parse as parse;
use super::ctx::Ctx;
use super::errors::Op;
use super::reply::Pool;
use super::rest::{self, Params, Req};

/// Наибольший размер страницы Earn — официально «Minimum is 10, maximum is 500» (api-3470273).
const EARN_PAGE_MAX: &str = "500";

/// Что делает: читает все счета параллельно.
/// Возвращает: счета по порядку: Спот, Фонд, Фьючерсы, Маржа (кросс), Маржа (изолированная), Опционы, Earn.
/// Ошибки: ошибка Get Account List — только если не прочитан ни один счёт.
pub async fn read(ctx: &Ctx) -> GatewayResult<Vec<AccountBalances>> {
    let v = ctx.venue();
    let (spot, fut, cross, iso, opt, earn) = tokio::join!(
        get(ctx, "/api/v1/accounts", vec![], Pool::Management),
        futures(ctx),
        get(
            ctx,
            "/api/v3/margin/accounts",
            vec![("queryType", "ALL".into())],
            Pool::Spot
        ),
        get(
            ctx,
            "/api/v3/isolated/accounts",
            vec![("queryType", "ALL".into())],
            Pool::Spot
        ),
        get(
            ctx,
            "/api/v1/accounts",
            vec![("type", "option".into())],
            Pool::Management
        ),
        get(
            ctx,
            "/api/v1/earn/hold-assets",
            vec![("pageSize", EARN_PAGE_MAX.into())],
            Pool::Earn
        ),
    );
    let rows = |name: &str, d: &GatewayResult<Value>, ty: &str| match d {
        Ok(d) => parse::spot_rows(&v, d, ty).map(|r| parse::account(name, r, None)),
        Err(e) => Err(e.clone()),
    };
    let out: Vec<AccountBalances> = vec![
        rows("Спот", &spot, "trade"),
        rows("Фонд (Funding)", &spot, "main"),
        Ok(fut),
        cross.and_then(|d| parse::cross_margin(&v, "Маржа (кросс)", &d)),
        iso.and_then(|d| parse::isolated(&v, "Маржа (изолированная)", &d)),
        rows("Опционы", &opt, "option"),
        earn.and_then(|d| parse::earn(&v, "Earn", &d)),
    ]
    .into_iter()
    .zip(NAMES)
    .map(|(r, name)| r.unwrap_or_else(|e| parse::failed(name, e.message)))
    .collect();
    if out.iter().all(|a| a.error.is_some()) {
        return Err(spot.err().unwrap_or_else(|| none_read(ctx)));
    }
    Ok(out)
}

/// Названия счетов в порядке read (для счёта, который не прочитан).
const NAMES: [&str; 7] = [
    "Спот",
    "Фонд (Funding)",
    "Фьючерсы",
    "Маржа (кросс)",
    "Маржа (изолированная)",
    "Опционы",
    "Earn",
];

/// Подписанный GET на основной домен. Принимает: путь, параметры, пул. Возвращает: data. Ошибки: как rest::signed.
async fn get(ctx: &Ctx, path: &str, q: Params, pool: Pool) -> GatewayResult<Value> {
    rest::signed(ctx, Method::GET, path, q, None, Op::Other, pool).await
}

/// Что делает: счёт «Фьючерсы» — по каждой монете маржи из venue.futures_balance_currencies запрос на домен
/// venue.futures_rest_url, параллельно. Возвращает: счёт; не задана настройка или не прочитана ни одна монета — error;
/// часть монет не прочитана — причина в note. Ошибок наружу нет.
async fn futures(ctx: &Ctx) -> AccountBalances {
    const NAME: &str = "Фьючерсы";
    let Ok(url) = ctx.text("venue.futures_rest_url") else {
        return parse::failed(
            NAME,
            "не задана venue.futures_rest_url — счёт не читается".into(),
        );
    };
    let urls = [url.trim_end_matches('/').to_string()];
    let curs = match ctx
        .settings
        .list("venue.futures_balance_currencies", ctx.at())
    {
        Ok(c) if !c.is_empty() => c,
        _ => {
            let msg = "не задана venue.futures_balance_currencies — счёт не читается";
            return parse::failed(NAME, msg.into());
        }
    };
    let replies = futures_util::future::join_all(curs.iter().map(|c| {
        let r = Req {
            method: Method::GET,
            path: "/api/v1/account-overview-all",
            query: vec![("currency", c.clone())],
            body: None,
            op: Op::Other,
            pool: Pool::Futures,
            signed: true,
            heavy: false,
        };
        rest::send_at(ctx, &urls, r)
    }))
    .await;
    let (mut rows, mut notes, mut fails) = (Vec::new(), Vec::new(), Vec::new());
    for (c, r) in curs.iter().zip(replies) {
        match r.and_then(|d| parse::futures_main(&ctx.venue(), &d)) {
            Ok(Some((b, n))) => {
                rows.push(b);
                notes.push(n);
            }
            Ok(None) => fails.push(format!(
                "{c}: в ответе нет основного счёта (accountName main)"
            )),
            Err(e) => fails.push(format!("{c}: {}", e.message)),
        }
    }
    if fails.len() == curs.len() {
        return parse::failed(NAME, fails.join("; "));
    }
    if !fails.is_empty() {
        notes.push(format!("не прочитано — {}", fails.join("; ")));
    }
    parse::account(NAME, rows, Some(notes.join("; ")))
}

/// Ошибка «не прочитан ни один счёт» (если у Get Account List своей ошибки нет). Ошибок нет.
fn none_read(ctx: &Ctx) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::BadResponse,
        None,
        "KuCoin: не прочитан ни один счёт",
    ))
}
