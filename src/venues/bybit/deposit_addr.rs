//! Файл: venues/bybit/deposit_addr.rs
//! Что делает: адрес депозита Bybit по монете и сети. Кэш в памяти по (монета, сеть) на время работы процесса (как
//! решение 118, Я-5 у MEXC: адрес нужен в предпроверке каждой сделки). Адрес с непустым tag — ошибка «требует tag — не
//! поддерживается» (урок 10, Я-15: контракт отдаёт только адрес — вывод без tag не зачислится); такой не кэшируется.
//! Кто вызывает: mod.rs (deposit_address из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официальная документация «Get Master Deposit Address» GET /v5/asset/deposit/query-address?coin=&chainType=
//! https://bybit-exchange.github.io/docs/v5/asset/deposit/master-deposit-addr — «Query the deposit address information
//! of MASTER account»; chainType — «Please use the value of >> chain from coin-info»; ответ: coin, chains[]: chain,
//! chainType, addressDeposit, tagDeposit. Лимит «300 req/min». Депозит зачисляется на FUND («Your funds will be
//! deposited into FUND wallet by default», Set Deposit Account) — перевод на торговый счёт: fund_move.rs.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Полученные адреса депозита: (монета, сеть) → адрес. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DepositAddrs {
    map: Arc<RwLock<HashMap<(String, String), String>>>,
}

/// Что делает: адрес депозита монеты в сети (chain) — из памяти или запросом.
/// Принимает: окружение, кэш, монету, сеть. Возвращает: адрес.
/// Ошибки: InvalidAddress — адреса в сети нет или он требует tag; связь, коды Bybit.
pub async fn deposit_address(
    ctx: &Ctx,
    addrs: &DepositAddrs,
    asset: &Asset,
    network: &str,
) -> GatewayResult<String> {
    let key = (asset.ticker.clone(), network.to_string());
    if let Some(a) = addrs
        .map
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(&key)
    {
        return Ok(a.clone());
    }
    let p = vec![
        ("coin", asset.ticker.clone()),
        ("chainType", network.to_string()),
    ];
    let data = rest::get(ctx, "/v5/asset/deposit/query-address", p, Op::Other).await?;
    let found = data.get("chains").and_then(|c| c.as_array()).and_then(|a| {
        a.iter()
            .find(|r| num::text_opt(r, "chain").as_deref() == Some(network))
    });
    let found = found.ok_or_else(|| {
        invalid(
            ctx,
            format!(
                "У {} нет адреса депозита Bybit в сети {network} — создайте его в кабинете Bybit (Deposit)",
                asset.ticker
            ),
        )
        .with_raw(data.to_string())
    })?;
    if let Some(tag) = num::text_opt(found, "tagDeposit").filter(|t| !t.trim().is_empty()) {
        return Err(invalid(
            ctx,
            format!(
                "Адрес депозита Bybit {} в сети {network} требует tag ({tag}) — не поддерживается",
                asset.ticker
            ),
        )
        .with_raw(found.to_string()));
    }
    let address = num::text(found, "addressDeposit")?;
    addrs
        .map
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, address.clone());
    Ok(address)
}

/// Ошибка InvalidAddress с площадкой Bybit.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
