//! Файл: venues/bybit/networks_parse.rs
//! Что делает: разбор ответа GET /v5/asset/coin/query-info в сети монет формата ядра (CoinNetworkInfo): сеть chain,
//! адрес контракта у сети BSC, статусы ввода/вывода, комиссия, минимумы, максимум, точность, подтверждения.
//! Кто вызывает: wallet.rs, tests/bybit_real_data.rs.
//! От чего зависит: num.rs, errors.rs, core/types.
//! Настройки: нет (название сети BSC передаёт wallet.rs из venue.bsc_network).
//!
//! Официальная документация «Get Coin Info» https://bybit-exchange.github.io/docs/v5/asset/coin-info — rows[]: coin,
//! chains[]: chain, chainType, confirmation («Number of confirmations for deposit»), withdrawFee («If withdraw fee is
//! empty, It means that this coin does not support withdrawal»), depositMin, withdrawMin, minAccuracy («The precision
//! of withdraw or deposit»), chainDeposit / chainWithdraw («0: suspend. 1: normal»), withdrawPercentageFee («e.g., 0.022
//! means 2.2%»), contractAddress («"" means no contract address»), withdrawMax («-1 indicates no limit»).
//! Процентная комиссия вывода (withdrawPercentageFee ≠ 0) в ядре не учитывается (там комиссия — число монет): у такой
//! сети комиссия вывода — «неизвестна» (None), ядро сделку с выводом по ней не делает; список — в журнал (вопрос Тиграну).

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, VenueId};
use crate::gateway::errors::GatewayResult;

use super::{errors, num};

/// Результат разбора.
#[derive(Debug, Default)]
pub struct Parsed {
    /// Сети формата ядра.
    pub list: Vec<CoinNetworkInfo>,
    /// Пропущенные сети: «монета/сеть: причина».
    pub skipped: Vec<String>,
    /// Сети BSC с процентной комиссией вывода: «монета=доля».
    pub percent_fee: Vec<String>,
}

/// Что делает: разбор coin/query-info. Принимает: result ответа, название сети BSC, площадку.
/// Возвращает: Parsed — сеть с неразборчивым полем исключается, остальные работают. Ошибка BadResponse — нет rows.
pub fn parse_networks(data: &Value, bsc: &str, venue: &VenueId) -> GatewayResult<Parsed> {
    let mut p = Parsed::default();
    for c in num::arr(data, "rows")? {
        let ticker = num::text(c, "coin")?;
        let Some(chains) = c.get("chains").and_then(Value::as_array) else {
            continue;
        };
        for n in chains {
            let Some(chain) = num::text_opt(n, "chain") else {
                continue;
            };
            match one(n, &ticker, &chain, chain == bsc, venue) {
                Ok((info, pct)) => {
                    if let Some(v) = pct.filter(|_| chain == bsc) {
                        p.percent_fee.push(format!("{ticker}={v}"));
                    }
                    p.list.push(info);
                }
                Err(e) => p.skipped.push(format!("{ticker}/{chain}: {}", e.message)),
            }
        }
    }
    Ok(p)
}

/// Одна сеть монеты. Возвращает: (сеть ядра, процентная комиссия, если не ноль). Ошибка BadResponse — поле не число.
fn one(
    n: &Value,
    ticker: &str,
    chain: &str,
    is_bsc: bool,
    venue: &VenueId,
) -> GatewayResult<(CoinNetworkInfo, Option<Decimal>)> {
    let contract = if is_bsc {
        num::text_opt(n, "contractAddress").and_then(|a| a.parse().ok())
    } else {
        None
    };
    let fee = num::dec_opt(n, "withdrawFee")?;
    let pct = num::dec_opt(n, "withdrawPercentageFee")?.filter(|v| !v.is_zero());
    let max = match num::dec_opt(n, "withdrawMax")? {
        Some(v) if v == Decimal::NEGATIVE_ONE => Some(Decimal::MAX),
        other => other,
    };
    let info = CoinNetworkInfo {
        venue: venue.clone(),
        asset: Asset {
            ticker: ticker.to_string(),
            bsc_contract: contract,
        },
        network: chain.to_string(),
        deposit_enabled: flag(n, "chainDeposit")?,
        // Пустая комиссия вывода — монета вывод не поддерживает (документация).
        withdraw_enabled: flag(n, "chainWithdraw")? && fee.is_some(),
        withdraw_fee: if pct.is_some() { None } else { fee },
        withdraw_min: num::dec_opt(n, "withdrawMin")?,
        withdraw_max: max,
        deposit_min: num::dec_opt(n, "depositMin")?,
        deposit_confirmations: num::text_opt(n, "confirmation").and_then(|s| s.parse().ok()),
        withdraw_precision: num::text_opt(n, "minAccuracy").and_then(|s| s.parse().ok()),
        withdraw_step: None,
        updated_at: Utc::now(),
    };
    Ok((info, pct))
}

/// Статус сети chainDeposit / chainWithdraw: "1" — normal, "0" — suspend. Иное — BadResponse (сеть пропускается).
pub fn flag(n: &Value, field: &str) -> GatewayResult<bool> {
    match num::text_opt(n, field).as_deref() {
        Some("1") => Ok(true),
        Some("0") => Ok(false),
        other => Err(errors::bad(
            &format!("coin/query-info: {field} не 0/1 ({other:?})"),
            &n.to_string(),
        )),
    }
}
