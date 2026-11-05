//! Файл: venues/htx/accounts.rs
//! Что делает: счета HTX — список всех счетов пользователя и id спот-счёта (account-id нужен ордеру, балансу спота и
//! потоку аккаунта). id спот-счёта запоминается на время процесса (не меняется у пользователя); нет спот-счёта — это
//! «счёт не открыт» (урок 27): балансы — пусто, ордер и вывод — отказ с причиной.
//! Кто вызывает: account.rs, all_balances.rs, order_rules.rs, user_stream_conn.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет.
//!
//! Официально — Get all Accounts of the Current User GET /v1/account/accounts («100 times/2s», право Read)
//! https://huobiapi.github.io/docs/spot/v1/en/#get-all-accounts-of-the-current-user: data[] {id, type, subtype,
//! state}; type: spot, margin, otc, point, super-margin, investment, borrow, grid-trading, deposit-earning,
//! otc-options; state: working, lock.

use std::sync::{Arc, RwLock};

use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::{num, rest};

/// Тип спот-счёта — значение type документации.
pub const SPOT: &str = "spot";
/// Счёт работает — значение state документации.
pub const WORKING: &str = "working";

/// Один счёт HTX.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub id: i64,
    pub kind: String,
    pub subtype: String,
    pub state: String,
}

/// Запомненный id спот-счёта. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct SpotId {
    id: Arc<RwLock<Option<i64>>>,
}

impl SpotId {
    /// Что делает: id спот-счёта — из памяти или запросом списка счетов.
    /// Возвращает: Some(id); None — спот-счёта нет («счёт не открыт»). Ошибки: ключи, связь, ответ HTX.
    pub async fn get(&self, ctx: &Ctx) -> GatewayResult<Option<i64>> {
        if let Some(id) = *self.id.read().unwrap_or_else(|p| p.into_inner()) {
            return Ok(Some(id));
        }
        let spot = list(ctx).await?.into_iter().find(|a| a.kind == SPOT);
        let id = spot.map(|a| a.id);
        if let Some(i) = id {
            *self.id.write().unwrap_or_else(|p| p.into_inner()) = Some(i);
        }
        Ok(id)
    }

    /// То же, но спот-счёт обязателен. Ошибка NotConfigured — спот-счёта нет (открыть в кабинете HTX).
    pub async fn require(&self, ctx: &Ctx) -> GatewayResult<i64> {
        self.get(ctx).await?.ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::NotConfigured,
                None,
                "У пользователя HTX нет спот-счёта (type spot в /v1/account/accounts) — счёт не открыт",
            ))
        })
    }
}

/// Все счета пользователя. Ошибки: ключи, связь, ответ HTX.
pub async fn list(ctx: &Ctx) -> GatewayResult<Vec<Account>> {
    let data = rest::get(ctx, "/v1/account/accounts", vec![]).await?;
    parse(&data)
}

/// Разбор data списка счетов. Ошибка BadResponse — не массив или нет id / type.
pub fn parse(data: &Value) -> GatewayResult<Vec<Account>> {
    let list = match data {
        Value::Null => return Ok(vec![]),
        Value::Array(a) => a,
        _ => {
            return Err(super::errors::bad(
                "accounts: data не массив",
                &data.to_string(),
            ));
        }
    };
    list.iter()
        .map(|a| {
            Ok(Account {
                id: num::int(a, "id")?,
                kind: num::text(a, "type")?,
                subtype: num::text_opt(a, "subtype").unwrap_or_default(),
                state: num::text_opt(a, "state").unwrap_or_default(),
            })
        })
        .collect()
}
