//! Файл: venues/kraken/deposit_addr.rs
//! Что делает: адрес депозита Kraken по монете и сети: сначала выданные адреса метода (список v2), нет годного —
//! новый адрес (claim) только при venue.deposit_address_claim = да (у метода предел неиспользованных адресов, обычно 5 —
//! не плодить). Кэш в памяти (адрес нужен в предпроверке каждой сделки — решение 118, урок 25) с учётом expire_time.
//! Адрес с tag/memo — ошибка «требует memo — не поддерживается»; такой не кэшируется.
//! Кто вызывает: mod.rs (deposit_address).
//! От чего зависит: rest.rs, num.rs, wallet.rs (method_id), methods.rs, ctx.rs.
//! Настройки: venue.deposit_address_claim [kraken] (необязательная), venue.funding_list_limit [kraken].
//!
//! Официально:
//! - GET /funding/v2/deposit/addresses (OpenAPI spot-rest.yaml, listFundingClaimedAddressesV2): scope[method_id], asset,
//!   limit 1–500, cursor; addresses[] — method_id, id, last_used, expire_time, address_details.crypto {address, tag, memo};
//! - PUT /funding/v1/deposit/address (claimFundingDepositAddressV1): тело {method_id} → address_details.crypto;
//! - гид https://docs.kraken.com/exchange/guides/rest/funding.md: «limited (typically limit: 5)», при пределе —
//!   TooManyDepositAddresses; неиспользованный старый адрес «begins expiring and is removed after 7 days»; «If tag or
//!   memo is present, include it with the address» — у бота memo не поддерживается.
//!   Счёт зачисления у Kraken один (спот) — перевода после депозита нет.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use reqwest::Method;
use serde_json::{Value, json};

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::methods::{CLASS, Dir};
use super::{num, rest, wallet};

/// Адрес и когда он истекает (None — срок не указан).
type Entry = (String, Option<DateTime<Utc>>);

/// Полученные адреса: (монета, сеть ядра) → адрес. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DepositAddrs {
    map: Arc<RwLock<HashMap<(String, String), Entry>>>,
}

impl DepositAddrs {
    /// Что делает: адрес депозита монеты в сети — из памяти (не истёк) или запросом.
    /// Принимает: окружение, монету, сеть ядра. Возвращает: адрес.
    /// Ошибки: InvalidAddress — адреса нет (а создавать запрещено) или он требует memo; связь, ответ Kraken.
    pub async fn get(&self, ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<String> {
        let key = (asset.ticker.clone(), network.to_string());
        if let Some((a, exp)) = self.map.read().unwrap_or_else(|p| p.into_inner()).get(&key)
            && exp.is_none_or(|e| e > Utc::now())
        {
            return Ok(a.clone());
        }
        let (method_id, _) = wallet::method(ctx, &asset.ticker, network, Dir::Deposit).await?;
        let limit = ctx.int("venue.funding_list_limit")?;
        let q = vec![
            rest::p("scope[method_id]", method_id.clone()),
            rest::p("asset[class]", CLASS),
            rest::p("asset[name]", asset.ticker.clone()),
            rest::p("limit", limit.to_string()),
        ];
        let v = rest::funding(
            ctx,
            Method::GET,
            "/funding/v2/deposit/addresses",
            q,
            None,
            Op::Other,
        )
        .await?;
        let picked =
            match pick(&v).map_err(|m| invalid(ctx, format!("{} {network}: {m}", asset.ticker)))? {
                Some(e) => e,
                None => claim(ctx, &asset.ticker, network, &method_id).await?,
            };
        self.map
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, picked.clone());
        Ok(picked.0)
    }
}

/// Выбор из выданных: не истёкший; без срока — раньше срочных, среди срочных — дальний срок.
/// Возвращает: адрес или None (годных нет). Ошибка — текст (у выбранного tag/memo, нет address).
pub fn pick(v: &Value) -> Result<Option<Entry>, String> {
    let now = Utc::now();
    let rows = v
        .get("addresses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut best: Option<(&Value, Option<DateTime<Utc>>)> = None;
    for r in &rows {
        let exp = num::text_opt(r, "expire_time")
            .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
            .map(|t| t.with_timezone(&Utc));
        if exp.is_some_and(|e| e <= now) {
            continue;
        }
        let better = match (&best, exp) {
            (None, _) => true,
            (Some((_, Some(_))), None) => true,
            (Some((_, Some(b))), Some(e)) => e > *b,
            _ => false,
        };
        if better {
            best = Some((r, exp));
        }
    }
    let Some((r, exp)) = best else {
        return Ok(None);
    };
    crypto(r.get("address_details")).map(|a| Some((a, exp)))
}

/// Адрес из address_details.crypto; tag/memo — ошибка. Ошибка — текст.
fn crypto(d: Option<&Value>) -> Result<String, String> {
    let c = d
        .and_then(|d| d.get("crypto"))
        .ok_or("в ответе нет address_details.crypto")?;
    for f in ["tag", "memo"] {
        if let Some(v) = num::text_opt(c, f).filter(|v| !v.trim().is_empty()) {
            return Err(format!("адрес требует {f} ({v}) — не поддерживается"));
        }
    }
    num::text_opt(c, "address").ok_or_else(|| "в ответе нет address".into())
}

/// Новый адрес (claim) — только при venue.deposit_address_claim = да. Ошибки: InvalidAddress — запрещено настройкой,
/// memo; TooManyDepositAddresses и прочие ответы Kraken.
async fn claim(ctx: &Ctx, ticker: &str, network: &str, method_id: &str) -> GatewayResult<Entry> {
    if !ctx.flag("venue.deposit_address_claim").unwrap_or(false) {
        return Err(invalid(
            ctx,
            format!(
                "У {ticker} в сети {network} нет выданного адреса депозита Kraken — создайте его в кабинете (Deposit) \
                 или включите venue.deposit_address_claim"
            ),
        ));
    }
    let body = json!({"method_id": method_id});
    let v = rest::funding(
        ctx,
        Method::PUT,
        "/funding/v1/deposit/address",
        vec![],
        Some(body),
        Op::Other,
    )
    .await?;
    let a = crypto(v.get("address_details"))
        .map_err(|m| invalid(ctx, format!("{ticker} {network}: {m}")))?;
    Ok((a, None))
}

/// Ошибка InvalidAddress с площадкой Kraken.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
