//! Файл: venues/bingx/all_balances.rs
//! Что делает: балансы ВСЕХ счетов BingX для панели «Балансы» (решение 144) — спот, фонд (Funding), фьючерсы USDT-M,
//! фьючерсы COIN-M, стандартные фьючерсы и обзор всех счетов (копитрейдинг, grid, Earn, C2C — только оценка в USDT).
//! Только чтение: ни переводов, ни ордеров. Запросы — параллельно; ошибка одного счёта уходит в его error, остальные
//! показываются. Разбор ответов — all_balances_parse.rs (чистые функции).
//! Кто вызывает: mod.rs (account_balances контракта), панель «Балансы» — раз в ~3 с, пока открыта.
//! От чего зависит: rest.rs (подпись, адреса, резерв), account.rs (спот), all_balances_parse.rs, ctx.rs.
//! Настройки: те же, что у rest.rs (venue.rest_url, venue.rest_fallback_url, venue.recv_window_ms, http.*).
//! Новых нет: все пути — на том же базовом адресе REST (api-ai-skills, references/base-urls.md: «All BingX OpenAPI
//! endpoints share the same base URL prefix»).
//!
//! Эндпоинты (все GET, подпись HMAC, право ключа «Read» — по docs-v3 у каждого, кроме стандартных фьючерсов, где
//! право не указано):
//! - /openApi/spot/v1/account/balance — спот, 5/с на UID (spot-account/api-reference.md, account.rs);
//! - /openApi/fund/v1/account/balance — фонд, 2/с на UID (spot-account/api-reference.md; docs-v3 «Query Fund
//!   Account Assets»);
//! - /openApi/swap/v3/user/balance — фьючерсы USDT-M, 5/с на UID (swap-account/api-reference.md; docs-v3 Swap →
//!   Account Endpoints → Query account data);
//! - /openApi/cswap/v1/user/balance — фьючерсы COIN-M, 5/с на UID (cswap-trade/api-reference.md);
//! - /openApi/contract/v1/balance — стандартные фьючерсы, лимит в документации не указан (standard-trade/api-reference.md);
//! - /openApi/account/v1/allAccountBalance — обзор всех счетов, 5/с на UID (fund-account/api-reference.md).
//!
//! Ссылки: https://github.com/BingX-API/api-ai-skills/tree/main/skills и https://bingx-api.github.io/docs-v3/
//!
//! Лимиты (панель — 1 вызов за 3 с = по 1 запросу каждого пути за 3 с, т.е. 0,33 запроса/с на путь):
//! спот 0,33 из 5/с (6,7 %), фонд 0,33 из 2/с (16,7 %), USDT-M 0,33 из 5/с, COIN-M 0,33 из 5/с, обзор 0,33 из 5/с
//! (по 6,7 %), стандартные фьючерсы — 0,33/с (лимит не опубликован). Лимиты BingX — на путь и UID; общий лимит IP групп
//! 2 и 3 снят (rate-limits.md, журнал изменений 2025-12-16). Торговля пользуется из этих путей только спотом
//! (balances), её запас — 4,67/с. Переводы (asset/v1/transfer, 2/с) — отдельный путь, панель его не трогает.

use reqwest::Method;
use rust_decimal::Decimal;

use crate::core::types::AccountBalances;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::all_balances_parse as parse;
use super::ctx::Ctx;
use super::errors::Op;
use super::{account, rest};

/// Пути и названия счетов — из официальной документации (см. шапку).
const FUND: &str = "/openApi/fund/v1/account/balance";
const USDT_M: &str = "/openApi/swap/v3/user/balance";
const COIN_M: &str = "/openApi/cswap/v1/user/balance";
const STANDARD: &str = "/openApi/contract/v1/balance";
const OVERVIEW: &str = "/openApi/account/v1/allAccountBalance";
/// accountType обзора, у которых есть свой подробный счёт выше: их оценка идёт в note этого счёта.
const DETAILED: [(&str, &str); 3] = [
    ("USDTMPerp", "Фьючерсы USDT-M"),
    ("coinMPerp", "Фьючерсы COIN-M"),
    ("stdFutures", "Стандартные фьючерсы"),
];

/// Что делает: читает все счета BingX параллельно.
/// Принимает: окружение. Возвращает: счета (спот, фонд, USDT-M, COIN-M, стандартные фьючерсы, затем счета обзора
/// с ненулевой оценкой без своего подробного запроса). Ошибка одного счёта — в его error.
/// Ошибки: только если не прочитан ни один счёт — ошибка спота (ключи, связь, ответ BingX).
pub async fn read(ctx: &Ctx) -> GatewayResult<Vec<AccountBalances>> {
    let venue = ctx.venue();
    let (spot, fund, usdt_m, coin_m, std, over) = tokio::join!(
        account::balances(ctx),
        get(ctx, FUND),
        get(ctx, USDT_M),
        get(ctx, COIN_M),
        get(ctx, STANDARD),
        get(ctx, OVERVIEW),
    );
    let spot_err = spot.as_ref().err().cloned();
    let mut out = vec![
        account_of(
            "Спот",
            spot.map(|b| {
                let rows = b
                    .into_iter()
                    .filter(|x| !x.free.is_zero() || !x.locked.is_zero())
                    .collect();
                (rows, None)
            }),
        ),
        account_of(
            "Фонд (Funding)",
            fund.and_then(|d| parse::free_locked(&venue, &d).map(|r| (r, None))),
        ),
        account_of(
            "Фьючерсы USDT-M",
            usdt_m.and_then(|d| parse::margin(&venue, &d)),
        ),
        account_of(
            "Фьючерсы COIN-M",
            coin_m.and_then(|d| parse::margin(&venue, &d)),
        ),
        account_of(
            "Стандартные фьючерсы",
            std.and_then(|d| parse::standard(&venue, &d)),
        ),
    ];
    match over.and_then(|d| parse::overview(&d)) {
        Ok(list) => merge_overview(&mut out, list),
        Err(e) => out.push(failed("Обзор всех счетов", &e)),
    }
    if let Some(e) = spot_err
        && out.iter().all(|a| a.error.is_some())
    {
        return Err(e);
    }
    Ok(out)
}

/// Подписанный GET без параметров (Op::Other: только чтение, при сбое сети — резервный адрес).
async fn get(ctx: &Ctx, path: &str) -> GatewayResult<serde_json::Value> {
    rest::signed(ctx, Method::GET, path, vec![], Op::Other).await
}

/// Счёт из результата разбора: успех — строки и сведения, ошибка — error с точной причиной.
fn account_of(
    name: &str,
    r: GatewayResult<(Vec<crate::core::types::Balance>, Option<String>)>,
) -> AccountBalances {
    match r {
        Ok((balances, note)) => AccountBalances {
            account: name.into(),
            balances,
            error: None,
            note,
        },
        Err(e) => failed(name, &e),
    }
}

/// Непрочитанный счёт. Нет права у ключа — так и сказано (по docs-v3 нужно право «Read»); иначе — текст ошибки BingX.
fn failed(name: &str, e: &GatewayError) -> AccountBalances {
    let error = if e.code == ErrorCode::InsufficientPermissions {
        format!(
            "нет права у ключа BingX для этого счёта (нужно «Чтение»): {}",
            e.message
        )
    } else {
        e.message.clone()
    };
    AccountBalances {
        account: name.into(),
        balances: vec![],
        error: Some(error),
        note: None,
    }
}

/// Оценка обзора: у подробного счёта — дописывается в его note; остальные типы с ненулевой оценкой — отдельный счёт
/// без строк по монетам (BingX отдаёт по ним только сумму в USDT).
fn merge_overview(out: &mut Vec<AccountBalances>, list: Vec<(String, Decimal)>) {
    for (kind, usdt) in list {
        let est = format!("оценка BingX ≈ {} USDT", usdt.normalize());
        if let Some((_, name)) = DETAILED.iter().find(|(k, _)| *k == kind) {
            if let Some(acc) = out
                .iter_mut()
                .find(|a| a.account == *name && a.error.is_none())
            {
                acc.note = Some(match acc.note.take() {
                    Some(n) => format!("{n}; {est}"),
                    None => est,
                });
            }
            continue;
        }
        if usdt.is_zero() {
            continue;
        }
        out.push(AccountBalances {
            account: parse::overview_name(&kind),
            balances: vec![],
            error: None,
            note: Some(format!("{est} (по монетам API BingX этот счёт не отдаёт)")),
        });
    }
}
