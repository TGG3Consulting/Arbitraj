//! Файл: venues/binance/networks_parse.rs
//! Что делает: разбор ответа config/getall Binance в сети монет формата ядра (CoinNetworkInfo): код сети, адрес
//! контракта у сети BSC, флаги ввода/вывода, комиссия, пределы, шаг суммы вывода, минимум депозита по решению человека.
//! Кто вызывает: wallet.rs (в spawn_blocking), tests/binance_real_data.rs.
//! От чего зависит: num.rs, errors.rs, core/types.
//! Настройки: нет (коды своих сетей — BSC и, решение 431, Arbitrum — и правило минимума депозита передаёт wallet.rs).
//!
//! Официально: GET /sapi/v1/capital/config/getall (Wallet «All Coins' Information», вес IP 10) — монета: coin,
//! networkList[]: network, name, depositEnable, withdrawEnable, withdrawFee, withdrawMin, withdrawMax,
//! withdrawIntegerMultiple (ошибка −4021 «сумма вывода должна быть кратна» — это шаг), depositDust (Changelog Wallet
//! 2021-09-03: «minimum creditable amount»), minConfirm («for balance credit»), unLockConfirm, withdrawTag («whether the coin requires
//! a memo to withdraw»), contractAddress, denomination («1 1MBABYDOGE = 1000000 BABYDOGE»), busy.
//! Лот своей сети относится к биржевому тикеру и адресу именно этой строки. Курсы других сетей не подставляются.

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, VenueId};
use crate::gateway::errors::GatewayResult;
use crate::gateway::own_nets::OwnNets;

use super::{errors, num};

/// Правило минимума депозита (решение 143, урок 21): depositDust как минимум — да/нет; нет минимума — считать 0.
#[derive(Debug, Clone, Copy, Default)]
pub struct DepositMin {
    /// venue.deposit_dust_as_min [binance].
    pub dust_as_min: bool,
    /// venue.deposit_min_absent_ok [binance].
    pub absent_ok: bool,
}

/// Результат разбора getall.
#[derive(Default)]
pub struct Parsed {
    pub list: Vec<CoinNetworkInfo>,
    pub(super) units: super::denomination::Units,
    /// Пропущенные сети: «монета/сеть: причина».
    pub skipped: Vec<String>,
}

/// Что делает: разбор getall. Принимает: ответ (массив монет), свои сети (решение 431: код BSC и коды других сетей —
/// gateway/own_nets.rs; «BSC» — только BSC), площадку, правило минимума депозита.
/// Возвращает: Parsed — сеть с неразборчивым полем исключается, остальные работают.
/// Ошибки: BadResponse — ответ не массив или у монеты нет coin.
pub fn parse_networks(
    data: &Value,
    own: impl Into<OwnNets>,
    venue: &VenueId,
    min: DepositMin,
) -> GatewayResult<Parsed> {
    let own = own.into();
    let coins = data
        .as_array()
        .ok_or_else(|| errors::bad("config/getall не массив", &data.to_string()))?;
    let mut p = Parsed::default();
    for c in coins {
        let ticker = num::text(c, "coin")?;
        let Some(list) = c.get("networkList").and_then(Value::as_array) else {
            continue;
        };
        for n in list {
            let Some(code) = num::text_opt(n, "network") else {
                continue;
            };
            let own = own.has(&code);
            let parsed = one(n, &ticker, &code, own, venue, min);
            if own {
                p.units
                    .record(&code, &ticker, parsed.as_ref().ok().map(|(i, u)| (i, *u)));
            }
            match parsed {
                Ok((i, _)) => p.list.push(i),
                Err(why) => p.skipped.push(format!("{ticker}/{code}: {why}")),
            }
        }
    }
    Ok(p)
}

/// Одна сеть монеты (`is_bsc` — своя сеть: адрес и проверка denomination). Ошибка — текст причины (сеть пропускается).
fn one(
    n: &Value,
    ticker: &str,
    code: &str,
    is_bsc: bool,
    venue: &VenueId,
    min: DepositMin,
) -> Result<(CoinNetworkInfo, Option<Decimal>), String> {
    let e = |e: crate::gateway::errors::GatewayError| e.message;
    let contract = if is_bsc {
        num::text_opt(n, "contractAddress").and_then(|a| a.trim().parse().ok())
    } else {
        None
    };
    let ratio = if is_bsc {
        super::denomination::per_raw(n)?
    } else {
        None
    };
    if ratio.is_some()
        && (contract.is_none_or(|a| a == alloy_primitives::Address::ZERO)
            || n["coin"].as_str() != Some(ticker))
    {
        return Err("denomination указана без подтверждения адреса и спотового тикера".into());
    }
    let dust = num::dec_opt(n, "depositDust").map_err(e)?;
    let deposit_min = match (min.dust_as_min, dust) {
        (true, Some(d)) => Some(d),
        _ if min.absent_ok => Some(Decimal::ZERO),
        _ => None,
    };
    Ok((
        CoinNetworkInfo {
            venue: venue.clone(),
            asset: Asset {
                chain: None,
                ticker: ticker.to_string(),
                contract,
            },
            network: code.to_string(),
            deposit_enabled: flag(n, "depositEnable")?,
            withdraw_enabled: flag(n, "withdrawEnable")?,
            withdraw_fee: num::dec_opt(n, "withdrawFee").map_err(e)?,
            use_net_withdrawal_budget: false,
            withdraw_fee_pct: None,
            withdraw_min: num::dec_opt(n, "withdrawMin").map_err(e)?,
            withdraw_max: max_limit(n, "withdrawMax")?,
            deposit_min,
            deposit_confirmations: num::text_opt(n, "minConfirm").and_then(|s| s.parse().ok()),
            withdraw_precision: None,
            withdraw_step: n
                .get("withdrawIntegerMultiple")
                .and_then(num::to_dec)
                .filter(|v| *v > Decimal::ZERO),
            updated_at: Utc::now(),
        },
        ratio,
    ))
}

/// Наибольшая сумма вывода; целое длиннее предела Decimal — Decimal::MAX (заведомо больше любой нашей суммы).
/// Ошибка — текст: не число.
fn max_limit(n: &Value, field: &str) -> Result<Option<Decimal>, String> {
    match num::dec_opt(n, field) {
        Ok(v) => Ok(v),
        Err(e) => {
            let raw = n.get(field).and_then(Value::as_str).unwrap_or_default();
            let digits = raw.split('.').next().unwrap_or_default();
            if digits.len() > 28 && digits.bytes().all(|b| b.is_ascii_digit()) {
                Ok(Some(Decimal::MAX))
            } else {
                Err(e.message)
            }
        }
    }
}

/// Флаг сети — boolean. Ошибка — текст (сеть пропускается).
fn flag(n: &Value, field: &str) -> Result<bool, String> {
    match n.get(field) {
        Some(Value::Bool(b)) => Ok(*b),
        other => Err(format!(
            "{field} не boolean ({})",
            other.map_or("нет поля".into(), Value::to_string)
        )),
    }
}
