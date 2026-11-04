//! Файл: venues/binance/deposit_addr.rs
//! Что делает: адрес депозита Binance по монете и сети. Кэш в памяти по (монета, сеть) на время работы процесса
//! (урок 25, решение 118 Я-5: адрес нужен в предпроверке каждой сделки — повторных запросов нет). Адрес с непустым tag
//! (memo) — ошибка «требует memo — не поддерживается» (вывод на такой адрес без memo не зачислится); не кэшируется.
//! Кто вызывает: mod.rs (deposit_address из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официально: GET /sapi/v1/capital/deposit/address (Wallet «Deposit Address (supporting network)», вес IP 10) —
//! coin (обязателен), network («If network is not send, return with default network of the coin»; значения — из
//! networkList config/getall); ответ: address, coin, tag, url.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use reqwest::Method;

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

/// Что делает: адрес депозита монеты в сети — из памяти или запросом.
/// Принимает: окружение, кэш, монету, код сети. Возвращает: адрес.
/// Ошибки: InvalidAddress — адреса нет или он требует memo; связь, коды Binance.
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
        ("network", network.to_string()),
    ];
    let path = "/sapi/v1/capital/deposit/address";
    let data = rest::signed(ctx, Method::GET, path, p, Op::Other).await?;
    if let Some(tag) = num::text_opt(&data, "tag").filter(|t| !t.trim().is_empty()) {
        return Err(invalid(
            ctx,
            format!(
                "Адрес депозита Binance {} в сети {network} требует memo ({tag}) — не поддерживается",
                asset.ticker
            ),
        )
        .with_raw(data.to_string()));
    }
    let address = num::text_opt(&data, "address").ok_or_else(|| {
        invalid(
            ctx,
            format!(
                "У {} нет адреса депозита Binance в сети {network} — создайте его в кабинете (Кошелёк → Депозит)",
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

/// Ошибка InvalidAddress с площадкой Binance. Ошибок нет.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
