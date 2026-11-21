//! Файл: venues/kraken/withdraw_addr.rs
//! Что делает: подготовка вывода Kraken: id сохранённого адреса (address_id) по адресу ядра — вывод у Kraken только на
//! адрес из адресной книги — и точная комиссия вывода на сумму (котировка с токеном на 5 минут).
//! Кто вызывает: transfers.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.funding_list_limit [kraken].
//!
//! Официально:
//! - GET /funding/v1/addresses (OpenAPI spot-rest.yaml, listFundingAddressesV1): scope[method_id], limit ≤ 500, cursor;
//!   addresses[] — address_id, scope, address_details.crypto {address, tag, memo}, name, verified (обязателен); гид
//!   https://docs.kraken.com/exchange/guides/rest/funding.md: «you can use any address returned by listing with that
//!   method's method_id». Адреса добавляет Тигран в кабинете Kraken: ключу бота право «Add withdrawal addresses» не
//!   даётся (docs/notes/kraken-analysis-2026-11-05.md, п. 3);
//! - GET /funding/v1/fees/{method_id} (calculateFundingFeesV1): amount (обязателен), fee_included; ответ fee,
//!   gross_amount, net_amount, fee_details, withdrawal_fee_token («Expires after 5 minutes»).

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Котировка комиссии вывода.
#[derive(Debug, Clone)]
pub struct FeeQuote {
    pub fee: Decimal,
    pub net: Decimal,
    pub gross: Decimal,
    pub token: Option<String>,
}

/// Что делает: address_id адреса в адресной книге метода. Принимает: окружение, method_id, адрес ядра.
/// Возвращает: (address_id, адрес как он сохранён в книге — для expected_address). Ошибки: InvalidAddress — адреса нет,
/// он не подтверждён или с tag/memo; связь, Kraken.
pub async fn address_id(
    ctx: &Ctx,
    method_id: &str,
    address: &str,
) -> GatewayResult<(String, String)> {
    let q = vec![
        rest::p("scope[method_id]", method_id),
        rest::p("limit", ctx.int("venue.funding_list_limit")?.to_string()),
    ];
    let v = rest::funding(
        ctx,
        Method::GET,
        "/funding/v1/addresses",
        q,
        None,
        Op::Other,
    )
    .await?;
    find(&v, address).map_err(|m| ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, m)))
}

/// Поиск адреса в ответе списка (EVM-адрес — без учёта регистра). Возвращает: (address_id, адрес из книги).
/// Ошибка — текст причины.
pub fn find(v: &Value, address: &str) -> Result<(String, String), String> {
    let rows = v
        .get("addresses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let found = rows.iter().find(|r| {
        r.get("address_details")
            .and_then(|d| d.get("crypto"))
            .and_then(|c| num::text_opt(c, "address"))
            .is_some_and(|a| a.eq_ignore_ascii_case(address))
    });
    let Some(r) = found else {
        return Err(format!(
            "Адреса {address} нет в адресной книге Kraken для этого метода вывода — добавьте его в кабинете Kraken \
             (Withdraw → адрес на сети метода)"
        ));
    };
    let c = r.get("address_details").and_then(|d| d.get("crypto"));
    for f in ["tag", "memo"] {
        if let Some(x) = c
            .and_then(|c| num::text_opt(c, f))
            .filter(|x| !x.trim().is_empty())
        {
            return Err(format!(
                "Адрес {address} в книге Kraken с {f} ({x}) — не поддерживается"
            ));
        }
    }
    if r.get("verified").and_then(Value::as_bool) != Some(true) {
        return Err(format!(
            "Адрес {address} в книге Kraken не подтверждён (verified = false)"
        ));
    }
    let stored = c
        .and_then(|c| num::text_opt(c, "address"))
        .unwrap_or_default();
    let id = num::text_opt(r, "address_id").ok_or("в записи адреса нет address_id")?;
    Ok((id, stored))
}

/// Что делает: точная комиссия вывода суммы (комиссия внутри суммы, fee_included = true). Принимает: method_id, сумму.
/// Возвращает: FeeQuote. Ошибки: связь, ответ Kraken, BadResponse — нет полей.
pub async fn quote(ctx: &Ctx, method_id: &str, amount: Decimal) -> GatewayResult<FeeQuote> {
    let path = format!("/funding/v1/fees/{method_id}");
    let q = vec![
        rest::p("amount", amount.normalize().to_string()),
        rest::p("fee_included", "true"),
    ];
    let v = rest::funding(ctx, Method::GET, &path, q, None, Op::Other).await?;
    parse_quote(&v)
}

/// Разбор котировки. Ошибка BadResponse — нет fee, net_amount или gross_amount.
pub fn parse_quote(v: &Value) -> GatewayResult<FeeQuote> {
    let amt = |k: &str| {
        v.get(k)
            .ok_or_else(|| super::errors::bad(&format!("fees: нет {k}"), &v.to_string()))
            .and_then(num::amount_of)
            .map(|(_, a)| a)
    };
    Ok(FeeQuote {
        fee: amt("fee")?,
        net: amt("net_amount")?,
        gross: amt("gross_amount")?,
        // Не обязателен в схеме ответа: нет — вывод с current_fee и max_fee = эта комиссия (transfers.rs).
        token: num::text_opt(v, "withdrawal_fee_token"),
    })
}
