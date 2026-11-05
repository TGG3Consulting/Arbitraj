//! Файл: venues/gate/deposit_hold.rs
//! Что делает: решение 166 (Тигран, 05.11.2026) — удержания вывода Gate: депозиты со статусом DEP_CREDITED («Deposit
//! Credited, Withdrawal Pending Unlock» — зачислено, вывод ещё не разблокирован). Торговать ими можно (deposits.rs
//! считает их зачисленными, решение 135), вывести — нет; правило вычета решает ядро (core/precheck_withdraw.rs).
//! Кто вызывает: mod.rs (withdraw_hold из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, core/types (WithdrawHold).
//! Настройки: нет своих.
//!
//! Официальная документация: API v4 → Wallet → «Retrieve deposit records» GET /wallet/deposits
//! https://www.gate.com/docs/developers/apiv4/en/ (SDK gateapi-python docs/WalletApi.md#list_deposits, модель
//! DepositRecord: status «DEP_CREDITED: Recharge to account, withdrawal is not unlocked»); без currency — все монеты,
//! без from — последние 7 дней, limit — «limited to 500 transactions». Поля: currency, amount, chain, status.

use reqwest::Method;
use serde_json::Value;

use crate::core::types::{HoldItem, WithdrawHold};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, rest};

/// Статус DEP_CREDITED — значение перечисления status документации Gate, не настройка.
const DEP_CREDITED: &str = "DEP_CREDITED";
/// Сколько записей просить — наибольшее по документации («limited to 500 transactions»).
const DEPOSITS_LIMIT: &str = "500";

/// Удержания вывода сейчас: депозиты DEP_CREDITED за окно API по умолчанию (7 дней), все монеты.
/// Возвращает: удержания (пусто — нет). Ошибки: ключ, права, связь, лимит, непонятный ответ.
pub async fn withdraw_hold(ctx: &Ctx) -> GatewayResult<WithdrawHold> {
    let p = vec![("limit", DEPOSITS_LIMIT.to_string())];
    let data = rest::signed(ctx, Method::GET, "/wallet/deposits", p, None, Op::Other).await?;
    parse(&data)
}

/// Разбор списка депозитов: только status = DEP_CREDITED. Ошибка BadResponse — не массив, нет currency / amount.
pub fn parse(data: &Value) -> GatewayResult<WithdrawHold> {
    let list = data
        .as_array()
        .ok_or_else(|| errors::bad("история депозитов не массив", &data.to_string()))?;
    let mut items = vec![];
    for rec in list
        .iter()
        .filter(|r| num::text_opt(r, "status").as_deref() == Some(DEP_CREDITED))
    {
        let asset = num::text(rec, "currency")?;
        let amount = num::dec(rec, "amount")?;
        let chain = num::text_opt(rec, "chain").unwrap_or_else(|| "не указано".into());
        items.push(HoldItem {
            reason: format!(
                "зачислено, вывод ещё не разблокирован Gate: {amount} {asset} (DEP_CREDITED, сеть {chain})"
            ),
            asset,
            amount,
        });
    }
    Ok(WithdrawHold::of(items))
}
