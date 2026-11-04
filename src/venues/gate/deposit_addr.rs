//! Файл: venues/gate/deposit_addr.rs
//! Что делает: адрес депозита Gate по монете и сети. Кэш в памяти по (монета, сеть) на время работы процесса (как
//! MEXC, решение 118, Я-5). Адрес с memo (payment_id) — ошибка «требует memo — не поддерживается» (Я-15: вывод без
//! memo не зачислится), такой не кэшируется; obtain_failed = 1 — адрес не получен, повторить позже. Минимум депозита
//! и подтверждения сети запоминаются для справочника сетей (wallet.rs, networks_parse::Extra).
//! Кто вызывает: mod.rs (deposit_address из контракта), deposit_info.rs (фоновое обновление минимума, решение 143).
//! От чего зависит: rest.rs, num.rs, ctx.rs, wallet.rs (Networks).
//! Настройки: нет своих.
//!
//! Официальная документация: API v4 → Wallet → «Generate currency deposit address» GET /wallet/deposit_address?currency=
//! https://www.gate.com/docs/developers/apiv4/en/ (SDK: docs/WalletApi.md#get_deposit_address, модели DepositAddress и
//! MultiChainAddressItem) — currency, address, min_deposit_amount, multichain_addresses[]: chain, address, payment_id
//! («A memo required when depositing certain currencies, such as a Tag or Memo»), payment_name («Tag» или «Memo»),
//! obtain_failed («0 means success; 1 means failure and retrieval may need to be retried»), min_confirms.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use reqwest::Method;
use serde_json::Value;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::wallet::Networks;
use super::{num, rest};

/// Полученные адреса депозита: (монета, сеть) → адрес. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DepositAddrs {
    map: Arc<RwLock<HashMap<(String, String), String>>>,
}

/// Что делает: адрес депозита монеты в сети — из памяти или запросом.
/// Принимает: окружение, кэш адресов, справочник сетей, монету, сеть (chain Gate).
/// Возвращает: адрес. Ошибки: InvalidAddress — адреса в сети нет, не получен или требует memo; связь, коды Gate.
pub async fn deposit_address(
    ctx: &Ctx,
    addrs: &DepositAddrs,
    nets: &Networks,
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
    fetch(ctx, addrs, nets, asset, network).await
}

/// Что делает: запрос адреса депозита всегда (без кэша): запоминает минимум депозита и подтверждения сети
/// (справочник сетей) и годный адрес. Вызывают deposit_address и фоновое обновление сведений депозита
/// (deposit_info.rs, решение 143). Принимает и возвращает — как deposit_address. Ошибки: как deposit_address.
pub async fn fetch(
    ctx: &Ctx,
    addrs: &DepositAddrs,
    nets: &Networks,
    asset: &Asset,
    network: &str,
) -> GatewayResult<String> {
    let key = (asset.ticker.clone(), network.to_string());
    let p = vec![("currency", asset.ticker.clone())];
    let data = rest::signed(
        ctx,
        Method::GET,
        "/wallet/deposit_address",
        p,
        None,
        Op::Other,
    )
    .await?;
    let min = num::dec_opt(&data, "min_deposit_amount").ok().flatten();
    let found = data
        .get("multichain_addresses")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|r| num::text_opt(r, "chain").as_deref() == Some(network))
        });
    let confirms = found
        .and_then(|r| num::text_opt(r, "min_confirms"))
        .and_then(|s| s.parse().ok());
    nets.note_deposit(&asset.ticker, min, network, confirms);
    let found = found.ok_or_else(|| {
        let m = format!("У {} нет адреса депозита Gate в сети {network} — создайте его в кабинете Gate (Депозит)", asset.ticker);
        invalid(ctx, m).with_raw(data.to_string())
    })?;
    if num::text_opt(found, "obtain_failed").as_deref() == Some("1") {
        let m = format!(
            "Gate не выдал адрес депозита {} в сети {network} (obtain_failed = 1) — повторить позже",
            asset.ticker
        );
        return Err(invalid(ctx, m).with_raw(found.to_string()));
    }
    if let Some(memo) = num::text_opt(found, "payment_id").filter(|m| !m.trim().is_empty()) {
        let kind = num::text_opt(found, "payment_name").unwrap_or_else(|| "memo".into());
        let m = format!(
            "Адрес депозита Gate {} в сети {network} требует {kind} ({memo}) — не поддерживается",
            asset.ticker
        );
        return Err(invalid(ctx, m).with_raw(found.to_string()));
    }
    let address = num::text(found, "address")?;
    addrs
        .map
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, address.clone());
    Ok(address)
}

/// Ошибка InvalidAddress с площадкой Gate.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
