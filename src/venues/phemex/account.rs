//! Файл: venues/phemex/account.rs
//! Что делает: балансы спот-счёта Phemex — все и одной монеты (решение 113), и кнопка «Проверить подключение».
//! Запроса «сведения о ключе» (права, IP, срок) у Phemex нет, поэтому проверка — пробными вызовами: баланс спота
//! (чтение), ставки комиссии, настройки вывода USDT; каждая причина отказа — точно, текстом Phemex.
//! Кто вызывает: mod.rs (функции контракта), all_balances.rs (Спот).
//! От чего зависит: rest.rs, num.rs, symbols.rs (масштабы), fees.rs, wallet.rs, ctx.rs.
//! Настройки: venue.quote_asset [phemex] (через fees.rs).
//!
//! Официально:
//! - Query wallets GET /spot/wallets?currency= (https://phemex-docs.github.io/#query-wallets): data [{currency,
//!   balanceEv, lockedTradingBalanceEv, lockedWithdrawEv, lastUpdateTimeNs}]; «GET /spot/wallets queries the wallets of
//!   all currencies». Доступно = balanceEv − lockedTradingBalanceEv − lockedWithdrawEv — так определено
//!   allAvailableBalanceRv в asset/info («total balance minus locked trading balance, locked withdraw balance and frozen
//!   balance»); что balanceEv — весь баланс с замороженным, прямо не сказано — проверить на живом ответе.
//! - Права ключа (Deposit And Withdraw → Endpoint Security Type): «Read & Trade & Withdraw permission, and IP Address
//!   is mandatory»; отдельного права «только спот» в документации нет (урок 19: денег на фьючерсном счёте не держать).

use reqwest::Method;
use serde_json::Value;

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{fees, num, rest, wallet};

/// Путь балансов спота.
pub const SPOT_WALLETS: &str = "/spot/wallets";

/// Балансы спот-счёта; coin — одна монета (None — все). Ошибки: ключи, связь, ответ Phemex, масштаб монеты.
pub async fn balances(
    ctx: &Ctx,
    symbols: &Symbols,
    coin: Option<&str>,
) -> GatewayResult<Vec<Balance>> {
    symbols.ensure_fresh(ctx).await?;
    let q = coin
        .map(|c| vec![("currency", c.to_string())])
        .unwrap_or_default();
    let data = rest::signed(ctx, Method::GET, SPOT_WALLETS, q, None, Op::Other).await?;
    spot_rows(ctx, symbols, &data)
}

/// Строки спот-счёта из data Query wallets: free = balance − lockedTrading − lockedWithdraw, locked = их сумма.
/// Ошибки: BadResponse — data не массив, нет полей, монеты нет в справочнике (масштаб неизвестен).
pub fn spot_rows(ctx: &Ctx, symbols: &Symbols, data: &Value) -> GatewayResult<Vec<Balance>> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("spot/wallets: data не массив", &data.to_string()))?;
    let scales = symbols.scales();
    let mut out = Vec::with_capacity(list.len());
    for b in list {
        let cur = num::text(b, "currency")?;
        let Some(&s) = scales.get(&cur) else {
            let all_zero = ["balanceEv", "lockedTradingBalanceEv", "lockedWithdrawEv"]
                .iter()
                .all(|f| b.get(*f).and_then(num::to_int).unwrap_or(0) == 0);
            if all_zero {
                continue;
            }
            return Err(symbols
                .scale(ctx, &cur)
                .err()
                .unwrap_or_else(|| super::errors::bad(&cur, "")));
        };
        let total = num::scaled(b, "balanceEv", s)?;
        let lock = num::scaled_opt(b, "lockedTradingBalanceEv", s)?.unwrap_or_default()
            + num::scaled_opt(b, "lockedWithdrawEv", s)?.unwrap_or_default();
        out.push(Balance {
            venue: ctx.venue(),
            asset: cur,
            free: total - lock,
            locked: lock,
        });
    }
    Ok(out)
}

/// Проверка подключения. Не падает: любая проблема — в details с ok=false.
pub async fn check(ctx: &Ctx, symbols: &Symbols) -> ConnectionCheck {
    match run(ctx, symbols).await {
        Ok(details) => ConnectionCheck { ok: true, details },
        Err(details) => ConnectionCheck { ok: false, details },
    }
}

/// Сама проверка. Возвращает: текст «Подключено…». Ошибка — точная причина.
async fn run(ctx: &Ctx, symbols: &Symbols) -> Result<String, String> {
    let rows = balances(ctx, symbols, None)
        .await
        .map_err(|e| format!("Баланс спота не прочитан: {}", e.message))?;
    let rates = fees::fetch(ctx)
        .await
        .map_err(|e| format!("Ставки комиссии не прочитаны: {}", e.message))?;
    let quote = ctx.text("venue.quote_asset").map_err(|e| e.message)?;
    // Проверка не трогает справочник сетей адаптера: сети кладутся во временный.
    let tmp = wallet::Networks::default();
    let wd = match wallet::coin_networks_of(ctx, &tmp, &quote).await {
        Ok(list) => format!(
            "настройки ввода и вывода {quote} прочитаны ({} сетей)",
            list.len()
        ),
        Err(e) => {
            return Err(format!(
                "Настройки вывода {quote} не прочитаны (нужно право Withdraw?): {}",
                e.message
            ));
        }
    };
    let nonzero = rows
        .iter()
        .filter(|b| !b.free.is_zero() || !b.locked.is_zero())
        .count();
    Ok(format!(
        "Подключено. Баланс спота прочитан (монет с остатком: {nonzero}), ставки комиссии — {} пар, {wd}. Права ключа и \
         белый список IP Phemex через API не отдаёт: ключ — «Read & Trade & Withdraw» с привязкой IP сервера (без IP \
         вывод не работает); вывод — только на адреса из «withdrawal address list» кабинета Phemex",
        rates.len()
    ))
}
