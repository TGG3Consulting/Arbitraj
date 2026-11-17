//! Файл: venues/kraken/deposit_addr_claim.rs
//! Что делает: решение 379 (Kraken, Тигран 17.11: «создание адреса депозита (claim) АВТОМАТИЧЕСКИ, без настройки») —
//! новый адрес депозита метода (claim), когда в списке выданных годного нет, и его подтверждение повторным списком:
//! адрес берётся ТОЛЬКО из списка v2 после claim (запись с тем же адресом, что вернул claim; итог claim неизвестен —
//! лучший годный из списка), как у Poloniex (gateway/addr_create.rs::confirm). Claim — класс операции Op::Create:
//! на резервный REST только если запрос точно не отправлен, таймаут — «итог неизвестен», затем тот же список.
//! Отказ TooManyDepositAddresses (предел неиспользованных адресов метода) — текст Kraken и пояснение по гиду; повтор не
//! на каждый сигнал — память неудачи (deposit_addr.rs, venue.networks_fresh_ms).
//! Кто вызывает: deposit_addr.rs (DepositAddrs::get).
//! От чего зависит: rest.rs, deposit_addr.rs (list, crypto, pick), ctx.rs (журнал), gateway/addr_create.rs.
//! Настройки: нет своих.
//!
//! Официально:
//! - PUT /funding/v1/deposit/address (OpenAPI spot-rest.yaml, claimFundingDepositAddressV1): тело {method_id} →
//!   address_details.crypto {address, tag, memo};
//! - гид https://docs.kraken.com/exchange/guides/rest/funding.md (прочитан 17.11.2026): «Claiming can reserve a new
//!   address, so list first and only claim when you need another one»; «If the method shares addresses, Kraken returns
//!   the shared address»; «Where address_generation is limited (typically limit: 5), you may hold up to five unused
//!   claimed addresses for that method at a time»; «If you are already at the limit, claim returns
//!   TooManyDepositAddresses. List existing addresses instead». HTTP-статус и вид тела этого отказа гид не называет;
//!   про повтор и идемпотентность claim — ни слова.

use reqwest::Method;
use serde_json::{Value, json};

use crate::gateway::addr_create::{Confirm, Failed, Failure, confirm, post_failure};
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::deposit_addr::{Entry, crypto, list, pick};
use super::errors::Op;
use super::{num, rest};

/// Имя отказа «предел адресов метода» в гиде Funding (строка поиска в тексте ответа — вид тела гид не описывает).
const TOO_MANY: &str = "TooManyDepositAddresses";

/// Что делает: запись списка v2 с адресом addr (сравнение — gateway/addr_create.rs::confirm). Принимает: ответ списка,
/// адрес. Возвращает: запись (адрес из списка и срок) или None. Ошибок нет (запись с memo — не подходит).
pub fn find(v: &Value, addr: &str) -> Option<Entry> {
    let rows = v.get("addresses")?.as_array()?;
    rows.iter().find_map(|r| {
        let a = crypto(r.get("address_details")).ok()?;
        if !matches!(confirm(Some(addr), Some(a.as_str())), Confirm::Use(_)) {
            return None;
        }
        let exp = num::text_opt(r, "expire_time")
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
            .map(|t| t.with_timezone(&chrono::Utc));
        Some((a, exp))
    })
}

/// Что делает: claim и подтверждение повторным списком (см. шапку).
/// Принимает: окружение, тикер, сеть ядра, method_id. Возвращает: адрес и срок из списка. После подтверждения — журнал
/// venue.deposit_address (Info, без адреса — как KuCoin, решение 208).
/// Ошибки (Failed — ошибка и вид, ревью 2 Н-А): отказ claim (текст Kraken; у TooManyDepositAddresses — пояснение),
/// адрес claim требует memo — Refused; итог claim неизвестен и список адреса не дал, claim принят, а адреса нет в
/// списке — Sent (Kraken, возможно, уже выдал адрес); ошибка повторного списка — по виду итога claim.
pub async fn claim(
    ctx: &Ctx,
    ticker: &str,
    network: &str,
    method_id: &str,
) -> Result<Entry, Failed> {
    let path = "/funding/v1/deposit/address";
    let body = json!({ "method_id": method_id });
    let posted = rest::funding(ctx, Method::PUT, path, vec![], Some(body), Op::Create)
        .await
        .map_err(|mut e| {
            let why = if e.message.contains(TOO_MANY) {
                " — предел неиспользованных адресов метода (гид Funding: «you may hold up to five unused claimed \
                 addresses for that method at a time», обычно 5; старый неиспользованный истекает через 7 дней после \
                 депозита на один из них)"
            } else {
                ""
            };
            e.message = format!(
                "Kraken: claim адреса депозита {ticker} в сети {network} (PUT {path}) не удался: {}{why}",
                e.message
            );
            e
        });
    let posted = match posted {
        Ok(v) => match crypto(v.get("address_details")) {
            Ok(a) => Ok(a),
            Err(m) => {
                let err = invalid(ctx, format!("Kraken {ticker} {network}: claim: {m}"));
                return Err(Failed {
                    err: Box::new(err),
                    kind: Failure::Refused,
                });
            }
        },
        Err(e) => Err(e),
    };
    // Вид неудачи, если адреса не будет: claim принят — Sent; ошибка claim — по её коду.
    let kind = match &posted {
        Ok(_) => Failure::Sent,
        Err(e) => post_failure(&e.code),
    };
    let v = list(ctx, ticker, method_id).await.map_err(|err| Failed {
        err: Box::new(err),
        kind,
    })?;
    let entry = match &posted {
        Ok(a) => find(&v, a),
        Err(_) => pick(&v).ok().flatten(),
    };
    match (entry, posted) {
        (Some(e), _) => {
            let m = format!(
                "Создан адрес депозита Kraken {ticker} в сети {network} (claim) — подтверждён повторным списком \
                 /funding/v2/deposit/addresses"
            );
            ctx.log(Level::Info, "venue.deposit_address", m).await;
            Ok(e)
        }
        (None, Err(e)) => Err(Failed {
            err: Box::new(e),
            kind,
        }),
        (None, Ok(_)) => Err(Failed {
            err: Box::new(invalid(
                ctx,
                format!(
                    "Адрес депозита Kraken {ticker} в сети {network}: claim принят, но адреса нет в повторном списке \
                     /funding/v2/deposit/addresses — адрес не принят, сделки нет"
                ),
            )),
            kind: Failure::Sent,
        }),
    }
}

/// Ошибка InvalidAddress с площадкой Kraken. Ошибок нет.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
