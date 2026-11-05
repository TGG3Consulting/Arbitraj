//! Файл: venues/poloniex/networks_parse.rs
//! Что делает: разбор справочника монет Poloniex v2 (GET /v2/currencies — весь, /v2/currencies/{currency} — одна монета)
//! в сети монет формата ядра (CoinNetworkInfo): blockchain — название сети ядра, адрес контракта у сети BSC, ввод/вывод,
//! комиссия и минимум вывода, подтверждения; имя монеты в сети (для адреса депозита) и нужен ли memo.
//! Кто вызывает: wallet.rs, deposit_addr.rs, tests/poloniex_real_data.rs.
//! От чего зависит: num.rs, errors.rs, core/types.
//! Настройки: нет (имя сети BSC и «минимум депозита неизвестен — 0» передаёт wallet.rs).
//!
//! Официально — Currency Information V2 https://api-docs.poloniex.com/spot/api/public/reference-data: coin, delisted,
//! tradeEnable, networkList[]: coin («Currency symbol» — имя монеты в этой сети), blockchain «Network the currency runs
//! on», currencyType («address» / «address-payment-id»), depositEnable, withdrawalEnable, withdrawFee, withdrawMin
//! («a value ≤ 0 means none»), decimals («Decimals of the currency»), minConfirm, depositAddress, contractAddress.
//!
//! Чего нет в документации (не угадывается): минимума депозита — None, а при venue.deposit_min_absent_ok = да — 0
//! (урок 21); точности суммы вывода — decimals назван «Decimals of the currency» (десятичные токена или точность
//! вывода — не сказано) → withdraw_precision = None (проверить на живом ответе и малом выводе); имени сети BSC —
//! venue.bsc_network по живому ответу. Монета delisted = true — ввод и вывод считаются закрытыми. Сеть с memo
//! (address-payment-id) — в список need_memo (депозит с memo не поддерживается).

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, VenueId};
use crate::gateway::errors::GatewayResult;

use super::num;

/// currencyType сети, которой нужен memo («address-payment-id»).
pub const PAYMENT_ID: &str = "address-payment-id";

/// Результат разбора.
#[derive(Default)]
pub struct Parsed {
    pub list: Vec<CoinNetworkInfo>,
    /// Сети BSC, которым нужен memo (currencyType = address-payment-id): «монета».
    pub need_memo: Vec<String>,
    /// Пропущенные сети: «монета/сеть: причина».
    pub skipped: Vec<String>,
}

/// Что делает: монеты ответа — массив (весь справочник) или один объект (одна монета). Ошибок нет.
pub fn coins(data: &Value) -> Vec<&Value> {
    match data {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![data],
        _ => vec![],
    }
}

/// Что делает: разбор справочника.
/// Принимает: ответ, имя сети BSC (venue.bsc_network), площадку, «минимум депозита неизвестен — 0».
/// Возвращает: Parsed. Ошибка BadResponse — ответ не массив и не объект или у монеты нет поля coin.
pub fn parse(data: &Value, bsc: &str, venue: &VenueId, min_ok: bool) -> GatewayResult<Parsed> {
    if !data.is_array() && !data.is_object() {
        return Err(super::errors::bad(
            "currencies: ответ не массив и не объект",
            "",
        ));
    }
    let mut p = Parsed::default();
    for c in coins(data) {
        let ticker = num::text(c, "coin")?;
        let delisted = num::flag(c, "delisted").unwrap_or(false);
        let Some(nets) = c.get("networkList").and_then(Value::as_array) else {
            continue;
        };
        for n in nets {
            let Some(chain) = num::text_opt(n, "blockchain") else {
                p.skipped.push(format!("{ticker}: сеть без blockchain"));
                continue;
            };
            let is_bsc = chain == bsc;
            match network(n, &ticker, &chain, is_bsc, delisted, venue, min_ok) {
                Ok(info) => {
                    if is_bsc && num::text_opt(n, "currencyType").as_deref() == Some(PAYMENT_ID) {
                        p.need_memo.push(ticker.clone());
                    }
                    p.list.push(info);
                }
                Err(e) => p.skipped.push(format!("{ticker}/{chain}: {}", e.message)),
            }
        }
    }
    Ok(p)
}

/// Одна сеть → CoinNetworkInfo. Ошибка BadResponse — поле не того вида.
fn network(
    n: &Value,
    ticker: &str,
    chain: &str,
    is_bsc: bool,
    delisted: bool,
    venue: &VenueId,
    min_ok: bool,
) -> GatewayResult<CoinNetworkInfo> {
    let contract = if is_bsc {
        num::text_opt(n, "contractAddress").and_then(|a| a.parse().ok())
    } else {
        None
    };
    Ok(CoinNetworkInfo {
        venue: venue.clone(),
        asset: Asset {
            ticker: ticker.to_string(),
            bsc_contract: contract,
        },
        network: chain.to_string(),
        deposit_enabled: !delisted && num::flag(n, "depositEnable")?,
        withdraw_enabled: !delisted && num::flag(n, "withdrawalEnable")?,
        withdraw_fee: num::dec_opt(n, "withdrawFee")?,
        withdraw_min: num::dec_opt(n, "withdrawMin")?.filter(|m| *m > Decimal::ZERO),
        withdraw_max: None,
        deposit_min: min_ok.then_some(Decimal::ZERO),
        deposit_confirmations: num::text_opt(n, "minConfirm").and_then(|s| s.parse().ok()),
        withdraw_precision: None,
        withdraw_step: None,
        updated_at: Utc::now(),
    })
}

/// Что делает: сеть монеты в ответе — имя монеты в сети (поле coin сети) и currencyType.
/// Принимает: ответ /v2/currencies/{currency}, тикер, сеть (blockchain). Возвращает: (coin сети, currencyType) или None.
pub fn chain_coin(data: &Value, ticker: &str, chain: &str) -> Option<(String, String)> {
    coins(data)
        .into_iter()
        .filter(|c| num::text_opt(c, "coin").is_some_and(|t| t.eq_ignore_ascii_case(ticker)))
        .filter_map(|c| c.get("networkList").and_then(Value::as_array))
        .flatten()
        .find(|n| num::text_opt(n, "blockchain").as_deref() == Some(chain))
        .map(|n| {
            (
                num::text_opt(n, "coin").unwrap_or_default(),
                num::text_opt(n, "currencyType").unwrap_or_default(),
            )
        })
}
