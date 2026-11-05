//! Файл: venues/bybit/withdrawable.rs
//! Что делает: решение 166 (Тигран, 05.11.2026) — сколько котируемой монеты Bybit сейчас держит от вывода: по счетам,
//! с которых бот выводит (venue.withdraw_account), удержано = «Available balance» − «Amount that can be withdrawn».
//! Bybit называет это задержкой вывода («delayed withdrawal»; отказ вывода 131085 «Withdrawal amount is greater than your
//! availale balance (the deplayed withdrawal is triggered)» — errors.rs).
//! Кто вызывает: mod.rs (withdraw_hold из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, core/types (WithdrawHold).
//! Настройки: venue.withdraw_account [bybit] (FUND, UTA или FUND,UTA — те же счета, что у вывода, transfers.rs).
//!
//! Официальная документация: «Get Withdrawable Amount» GET /v5/asset/withdraw/withdrawable-amount
//! https://bybit-exchange.github.io/docs/v5/asset/balance/delay-amount — coin* («Coin name, uppercase only»); ответ
//! result: limitAmountUsd («The frozen amount due to risk, in USD»), withdrawableAmount {SPOT, FUND, UTA, EARN} — у
//! каждого счёта coin, withdrawableAmount «Amount that can be withdrawn», availableBalance «Available balance»; счёта нет
//! в ответе — его нет у аккаунта (SPOT «not returned if spot wallet is removed»). limitAmountUsd не вычитается: в
//! примере документации он больше всех балансов (595051.7 USD) — смысл как «удержано сейчас» не подтверждён; показывается
//! в тексте причины. Сверить на живом ответе.

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{HoldItem, WithdrawHold};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, rest};

/// Удержание вывода котируемой монеты. Принимает: монету (USDT). Возвращает: удержания по счетам вывода (пусто — всё
/// доступное можно вывести). Ошибки: venue.withdraw_account не задана, ключ, связь, лимит, непонятный ответ.
pub async fn withdraw_hold(ctx: &Ctx, quote: &str) -> GatewayResult<WithdrawHold> {
    let accounts = ctx.text("venue.withdraw_account")?;
    let p = vec![("coin", quote.to_string())];
    let data = rest::get(ctx, "/v5/asset/withdraw/withdrawable-amount", p, Op::Other).await?;
    parse(&data, &accounts)
}

/// Разбор result. Принимает: result, счета вывода через запятую (как venue.withdraw_account). Возвращает: по одному
/// удержанию на счёт, где доступно больше, чем можно вывести. Ошибки: BadResponse — нет withdrawableAmount, у счёта
/// нет чисел или монеты.
pub fn parse(data: &Value, accounts: &str) -> GatewayResult<WithdrawHold> {
    let wallets = data
        .get("withdrawableAmount")
        .and_then(Value::as_object)
        .ok_or_else(|| errors::bad("нет withdrawableAmount", &data.to_string()))?;
    let risk = num::text_opt(data, "limitAmountUsd").unwrap_or_else(|| "не указано".into());
    let mut items = vec![];
    for name in accounts.split(',').map(str::trim).filter(|a| !a.is_empty()) {
        let Some(w) = wallets.get(name) else {
            continue;
        };
        let asset = num::text(w, "coin")?;
        let avail = num::dec(w, "availableBalance")?;
        let can = num::dec(w, "withdrawableAmount")?;
        let held = avail - can;
        if held <= Decimal::ZERO {
            continue;
        }
        items.push(HoldItem {
            reason: format!(
                "Bybit держит от вывода {held} {asset} на счёте {name} (доступно {avail}, можно вывести {can}; \
                 limitAmountUsd {risk})"
            ),
            asset,
            amount: held,
        });
    }
    Ok(WithdrawHold::of(items))
}
