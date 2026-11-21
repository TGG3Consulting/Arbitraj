//! Файл: venues/okx/all_balances.rs
//! Что делает: балансы ВСЕХ счетов OKX для панели «Балансы» (решение 144) — только чтение: Торговый (Trading — на нём
//! ордера бота; в режиме Spot отдельного фьючерсного счёта нет), Фонд (Funding — депозиты и вывод), Earn (Simple Earn) и
//! итоговая оценка по счетам. Запросы — параллельно; сбой одного счёта — в его error, остальные показываются. Пустой
//! ответ — пустой счёт, не ошибка (урок 27: «Only asset information of a currency with a balance greater than 0 will be
//! returned»).
//! Кто вызывает: mod.rs (Venue::account_balances), панель «Балансы» ≈ раз в 3 с, пока открыта.
//! От чего зависит: rest.rs, num.rs, account.rs (разбор торгового), ctx.rs.
//! Настройки: нет своих.
//!
//! Официально (https://www.okx.com/docs-v5/en/):
//! - «Trading Account → Get balance» GET /api/v5/account/balance (10 / 2 с) — account.rs;
//! - «Funding Account → Get balance» GET /api/v5/asset/balances (6 / с): bal «Balance», frozenBal, availBal;
//! - «Financial Product → Simple earn flexible → GET / Saving balance» GET /api/v5/finance/savings/balance: ccy, amt
//!   «Currency amount», earnings, loanAmt «Lending amount», pendingAmt «Pending amount»;
//! - «Funding Account → Get account asset valuation» GET /api/v5/asset/asset-valuation (1 / с): totalBal, details —
//!   funding, trading, classic (устарел), earn; единица по умолчанию — BTC.
//!
//! Раз в 3 с: 1 запрос на каждый эндпоинт — в пределах всех лимитов (самый строгий — оценка, 1 / с).

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{AccountBalances, Balance};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::{account, num, rest};

/// Что делает: читает все счета параллельно.
/// Возвращает: счета по порядку: Торговый, Фонд, Earn, Итог. Ошибки: только если не прочитан ни один счёт.
pub async fn read(ctx: &Ctx) -> GatewayResult<Vec<AccountBalances>> {
    let (trade, fund, earn, val) = tokio::join!(
        account::balances(ctx, None),
        rest::get(ctx, "/api/v5/asset/balances", vec![]),
        rest::get(ctx, "/api/v5/finance/savings/balance", vec![]),
        rest::get(ctx, "/api/v5/asset/asset-valuation", vec![]),
    );
    let first_err = trade.as_ref().err().cloned();
    let out = vec![
        trade.map(|r| account_of("Торговый (Trading)", r, None)),
        fund.and_then(|d| funding(ctx, &d)),
        earn.and_then(|d| savings(ctx, &d)),
        val.and_then(|d| valuation(&d)),
    ];
    let names = [
        "Торговый (Trading)",
        "Фонд (Funding)",
        "Earn (Simple Earn)",
        "Итог (оценка)",
    ];
    let out: Vec<AccountBalances> = out
        .into_iter()
        .zip(names)
        .map(|(r, name)| r.unwrap_or_else(|e| failed(name, e.message)))
        .collect();
    if out.iter().all(|a| a.error.is_some()) {
        return Err(first_err.unwrap_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                "OKX: не прочитан ни один счёт",
            ))
        }));
    }
    Ok(out)
}

/// Счёт без ошибки (нулевые строки отбрасываются). Ошибок нет.
fn account_of(name: &str, rows: Vec<Balance>, note: Option<String>) -> AccountBalances {
    AccountBalances {
        account: name.to_string(),
        balances: rows
            .into_iter()
            .filter(|b| !b.free.is_zero() || !b.locked.is_zero())
            .collect(),
        error: None,
        note,
    }
}

/// Счёт, который не прочитан. Ошибок нет.
fn failed(name: &str, reason: String) -> AccountBalances {
    AccountBalances {
        account: name.to_string(),
        balances: Vec::new(),
        error: Some(reason),
        note: None,
    }
}

/// Фонд: free = availBal, locked = frozenBal. Ошибки: BadResponse — data не массив или нет полей.
pub fn funding(ctx: &Ctx, d: &Value) -> GatewayResult<AccountBalances> {
    let list = d
        .as_array()
        .ok_or_else(|| super::errors::bad("asset/balances: data не массив", &d.to_string()))?;
    let rows = list
        .iter()
        .map(|b| {
            Ok(Balance {
                venue: ctx.venue(),
                asset: num::text(b, "ccy")?,
                free: num::dec_opt(b, "availBal")?.unwrap_or_default(),
                locked: num::dec_opt(b, "frozenBal")?.unwrap_or_default(),
            })
        })
        .collect::<GatewayResult<Vec<_>>>()?;
    Ok(account_of("Фонд (Funding)", rows, None))
}

/// Simple Earn: всё в продукте — locked = amt, свободного нет; в сведениях — суммы в ожидании. Ошибки: BadResponse.
pub fn savings(ctx: &Ctx, d: &Value) -> GatewayResult<AccountBalances> {
    let list = d
        .as_array()
        .ok_or_else(|| super::errors::bad("savings/balance: data не массив", &d.to_string()))?;
    let (mut rows, mut notes) = (Vec::new(), Vec::new());
    for b in list {
        let ccy = num::text(b, "ccy")?;
        if let Some(p) = num::dec_opt(b, "pendingAmt")?.filter(|p| !p.is_zero()) {
            notes.push(format!("в ожидании {p} {ccy}"));
        }
        rows.push(Balance {
            venue: ctx.venue(),
            asset: ccy,
            free: Decimal::ZERO,
            locked: num::dec_opt(b, "amt")?.unwrap_or_default(),
        });
    }
    let note = (!notes.is_empty()).then(|| notes.join("; "));
    Ok(account_of("Earn (Simple Earn)", rows, note))
}

/// Итог: оценка по счетам (в BTC по умолчанию) — только сведения, строк нет. Ошибки: BadResponse — нет данных.
pub fn valuation(d: &Value) -> GatewayResult<AccountBalances> {
    let r = rest::first(d, "asset-valuation")?;
    let det = r.get("details").cloned().unwrap_or(Value::Null);
    let part = |k: &str| num::text_opt(&det, k).unwrap_or_else(|| "—".into());
    let note = format!(
        "всего {} BTC: Funding {}, Trading {}, Earn {}",
        num::text_opt(r, "totalBal").unwrap_or_else(|| "—".into()),
        part("funding"),
        part("trading"),
        part("earn")
    );
    Ok(account_of("Итог (оценка)", vec![], Some(note)))
}
