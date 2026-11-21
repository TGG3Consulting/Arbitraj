//! Файл: venues/bybit/all_balances.rs
//! Что делает: балансы ВСЕХ счетов Bybit для панели «Балансы» (решение 144) — только чтение: единый торговый счёт
//! (UNIFIED: спот, маржа и деривативы — на UTA это один счёт; классических счетов CONTRACT/SPOT Bybit больше не даёт),
//! Фонд (FUND) и прочие счета из Asset Overview (Earn, торговые боты, копитрейдинг, криптозаймы, Launchpool, TradFi,
//! Alpha, stocks у UTA и т.д.). Три запроса — параллельно; сбой одного — в его error, остальные показываются.
//! Кто вызывает: mod.rs (Venue::account_balances), панель «Балансы» ≈ раз в 3 с, пока открыта.
//! От чего зависит: rest.rs (подпись, адрес venue.rest_url, пауза по X-Bapi-Limit* и 403 — reply.rs),
//! all_balances_parse.rs (разбор), ctx.rs.
//! Настройки: новых нет — все три эндпоинта на основном адресе REST (venue.rest_url); как у rest.rs.
//!
//! Официальная документация Bybit V5 (исходник github.com/bybit-exchange/docs, коммит d5a43d2 от 2026-11-04):
//! - «Get Wallet Balance» GET /v5/account/wallet-balance?accountType=UNIFIED
//!   https://bybit-exchange.github.io/docs/v5/account/wallet-balance — без coin: «returns non-zero asset info».
//! - «Get All Coins Balance» GET /v5/asset/transfer/query-account-coins-balance?accountType=FUND
//!   https://bybit-exchange.github.io/docs/v5/asset/balance/all-balance — без coin: «Query all coins» (coin обязателен
//!   только для UNIFIED). enum accountType: только UNIFIED и FUND (https://bybit-exchange.github.io/docs/v5/enum#accounttype).
//! - «Asset Overview» GET /v5/asset/asset-overview https://bybit-exchange.github.io/docs/v5/asset/balance/asset-overview
//!   — без accountType: «Returns all»; «does not support any regional account, e.g., Bybit KZ, GE, EU» — тогда ошибка
//!   Bybit идёт в error строки «Прочие счета».
//!
//! Права ключа для этих трёх эндпоинтов документация не называет (у Get API Key Information прямо: «Any permission»);
//! отказ (10005 «Permission denied») показывается в error счёта.
//!
//! Лимиты («Rate Limit Rules» https://bybit-exchange.github.io/docs/v5/rate-limit): лимит API — «per second per UID»
//! для каждого эндпоинта отдельно: wallet-balance 50/s, query-account-coins-balance 5/s, asset-overview 50/s; IP —
//! «600 requests within a 5-second window». Панель раз в 3 с = по 1 запросу каждого за 3 с: 0,33/s из 50/s (0,7 %;
//! этот же эндпоинт — balance()/balances() торговли, им остаётся > 49/s), 0,33/s из 5/s (6,7 %; торговля бота его не
//! вызывает), 0,33/s из 50/s (0,7 %; торговля не вызывает); IP — 5 запросов за 5 с из 600 (0,8 %). Лимиты ордеров
//! (/v5/order/*) — отдельные, панель их не трогает.

use serde_json::Value;

use crate::core::types::AccountBalances;
use crate::gateway::errors::GatewayResult;

use super::all_balances_parse as parse;
use super::ctx::Ctx;
use super::errors::Op;
use super::rest;

const PATH_WALLET: &str = "/v5/account/wallet-balance";
const PATH_COINS: &str = "/v5/asset/transfer/query-account-coins-balance";
const PATH_OVERVIEW: &str = "/v5/asset/asset-overview";

/// Что делает: читает все счета параллельно.
/// Возвращает: счета по порядку: единый торговый (UNIFIED), Фонд (FUND), затем счета из Asset Overview.
/// Ошибки: ошибка wallet-balance — только если не прочитан ни один счёт (ключи заблокированы, связь и т.п.).
pub async fn read(ctx: &Ctx) -> GatewayResult<Vec<AccountBalances>> {
    let v = ctx.venue();
    let (uta, fund, over) = tokio::join!(
        get(ctx, PATH_WALLET, "UNIFIED"),
        get(ctx, PATH_COINS, "FUND"),
        rest::get(ctx, PATH_OVERVIEW, vec![], Op::Other),
    );
    if let (Err(e), Err(_), Err(_)) = (&uta, &fund, &over) {
        return Err(e.clone());
    }
    let mut out = vec![
        one(parse::UNIFIED, uta.and_then(|d| parse::unified(&v, &d))),
        one(parse::FUND, fund.and_then(|d| parse::fund(&v, &d))),
    ];
    match over.and_then(|d| parse::overview(&v, &d)) {
        Ok(rest) => out.extend(rest),
        Err(e) => out.push(parse::failed(parse::OTHERS, e.message)),
    }
    Ok(out)
}

/// Подписанный GET с accountType. Принимает: путь, тип счёта. Возвращает: result. Ошибки: как rest::get.
async fn get(ctx: &Ctx, path: &str, account_type: &str) -> GatewayResult<Value> {
    rest::get(
        ctx,
        path,
        vec![("accountType", account_type.to_string())],
        Op::Other,
    )
    .await
}

/// Счёт или строка с ошибкой. Принимает: название (для ошибки), результат разбора. Ошибок нет.
fn one(name: &str, r: GatewayResult<AccountBalances>) -> AccountBalances {
    r.unwrap_or_else(|e| parse::failed(name, e.message))
}
