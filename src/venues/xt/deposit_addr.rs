//! Файл: venues/xt/deposit_addr.rs
//! Что делает: адрес депозита XT.com по монете и сети. Кэш в памяти по (монета, сеть) на время работы процесса
//! (урок 25, решение 118 Я-5: адрес нужен в предпроверке каждой сделки — повторных запросов нет). Адрес с непустым tag
//! (memo) — ошибка «требует memo — не поддерживается» (вывод на такой адрес без memo не зачислится); не кэшируется.
//! Кто вызывает: mod.rs (deposit_address из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официально: GET /v4/deposit/address (https://doc.xt.com/docs/spot/Deposit&Withdrawal/GetDepositAddress,
//! `_deposit_withdrawal/depositAddressGet.md`) — chain и currency обязательны (значения — из support/currency); ответ
//! result {address, memo}. Коды — errorCode.md (DEPOSIT_001…008).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use reqwest::Method;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::{Ctx, currency};
use super::errors::Op;
use super::{num, rest};

/// Полученные адреса депозита: (монета, сеть) → адрес. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DepositAddrs {
    map: Arc<RwLock<HashMap<(String, String), String>>>,
}

/// Что делает: адрес депозита монеты в сети — из памяти или запросом.
/// Принимает: окружение, кэш, монету, код сети. Возвращает: адрес.
/// Ошибки: InvalidAddress — адреса нет, он ещё создаётся или требует memo; связь, коды XT.
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
        ("chain", network.to_string()),
        ("currency", currency(&asset.ticker)),
    ];
    let data = match rest::signed(ctx, Method::GET, "/v4/deposit/address", p, None, Op::Other).await
    {
        Ok(d) => d,
        Err(e) if e.message.contains("DEPOSIT_006") || e.message.contains("DEPOSIT_007") => {
            let why = format!(
                "Адреса депозита XT {} в сети {network} пока нет ({}) — XT его создаёт, повторите позже",
                asset.ticker, e.message
            );
            return Err(invalid(ctx, why));
        }
        Err(e) => return Err(e),
    };
    if let Some(memo) = num::text_opt(&data, "memo").filter(|t| !t.trim().is_empty()) {
        return Err(invalid(
            ctx,
            format!(
                "Адрес депозита XT {} в сети {network} требует memo ({memo}) — не поддерживается",
                asset.ticker
            ),
        )
        .with_raw(data.to_string()));
    }
    let address = num::text_opt(&data, "address").ok_or_else(|| {
        invalid(
            ctx,
            format!(
                "У {} нет адреса депозита XT в сети {network} — создайте его в кабинете (Активы → Депозит)",
                asset.ticker
            ),
        )
        .with_raw(data.to_string())
    })?;
    addrs
        .map
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, address.clone());
    Ok(address)
}

/// Ошибка InvalidAddress с площадкой XT. Ошибок нет.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
