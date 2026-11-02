//! Файл: venues/bingx/deposit_addr.rs
//! Что делает: адрес депозита BingX по монете и сети. Кэш в памяти по (монета, сеть) на время работы процесса
//! (решение 118, Я-5: адрес нужен в предпроверке каждой сделки — повторно запросов нет). Адрес с непустым tag (memo)
//! — ошибка «требует memo — не поддерживается» (Я-15: вывод на такой адрес без memo не зачислится); такой не кэшируется.
//! Кто вызывает: mod.rs (deposit_address из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет.
//!
//! Официальная документация (docs-v3 и api-ai-skills/spot-wallet): GET /openApi/wallets/v1/capital/deposit/address —
//! data.data[]: coin, network, address, tag, status (0 — адрес активирован, 1 — запрошен, 2 — не запрошен).

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
/// Принимает: окружение, кэш адресов, монету, сеть (как в справочнике BingX).
/// Возвращает: адрес.
/// Ошибки: InvalidAddress — адреса в сети нет или адрес требует tag (memo); связь, коды BingX.
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
    let data = rest::signed(
        ctx,
        Method::GET,
        "/openApi/wallets/v1/capital/deposit/address",
        vec![("coin", asset.ticker.clone())],
        Op::Other,
    )
    .await?;
    let list = num::arr(&data, "data")?;
    let found = list
        .iter()
        // status по docs-v3: 0 — адрес активирован, 1 — запрошен, 2 — не запрошен. Нет поля — как в примере docs-v3.
        .find(|a| {
            num::text_opt(a, "network").as_deref() == Some(network)
                && num::text_opt(a, "status").is_none_or(|s| s == "0")
        });
    let found = found.ok_or_else(|| {
        invalid(
            ctx,
            format!("У {} нет адреса депозита в сети {network}", asset.ticker),
        )
    })?;
    if let Some(tag) = num::text_opt(found, "tag").filter(|t| !t.trim().is_empty()) {
        return Err(invalid(
            ctx,
            format!(
                "Адрес депозита BingX {} в сети {network} требует memo (tag {tag}) — не поддерживается",
                asset.ticker
            ),
        )
        .with_raw(found.to_string()));
    }
    let address = num::text(found, "address")?;
    addrs
        .map
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, address.clone());
    Ok(address)
}

/// Ошибка InvalidAddress с площадкой BingX.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
