//! Файл: venues/mexc/networks_parse.rs
//! Что делает: разбор ответа config/getall MEXC в сети монет формата ядра (CoinNetworkInfo): код netWork ↔ имя
//! network, адрес контракта у сети BSC, флаги ввода/вывода, комиссия, пределы, шаг суммы вывода.
//! Кто вызывает: wallet.rs (coin_networks), tests/mexc_real_data.rs.
//! От чего зависит: num.rs, errors.rs, symbols.rs (точность вывода), core/types.
//! Настройки: нет (название сети BSC и «минимум депозита неизвестен — 0» передаёт wallet.rs).
//!
//! Официальная документация: GET /api/v3/capital/config/getall
//! https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/query-the-currency-information — networkList: network
//! («Withdraw network display name (legacy parameter, will be deprecated)»), netWork («Withdraw network (new parameter,
//! for the new withdraw endpoint)»), contract, depositEnable, withdrawEnable, withdrawFee, withdrawMin, withdrawMax,
//! minConfirm, withdrawIntegerMultiple («Withdraw amount increment»). Минимального депозита в ответе нет.
//!
//! withdrawIntegerMultiple (решение 118, А-🟠7): в таблице — «Withdraw amount increment» (шаг), а в примере ответа той
//! же страницы — "18" (похоже на число знаков, а не на шаг). Противоречие документации; живого ответа в записях нет.
//! Безопасно: дробное значение 0 < v < 1 — однозначно шаг, сумма вывода вниз до кратного (ядро, floor_precision);
//! целое (≥ 1) — смысл не подтверждён, шаг не ставится (как было), такие сети BSC — в журнал (вопрос Тиграну).

use std::collections::HashMap;

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, VenueId};
use crate::gateway::errors::GatewayResult;

use super::symbols::Symbols;
use super::{errors, num};

/// Результат разбора getall.
pub struct Parsed {
    /// Сети формата ядра.
    pub list: Vec<CoinNetworkInfo>,
    /// (монета, код netWork) → отображаемое имя network.
    pub names: HashMap<(String, String), String>,
    /// Пропущенные сети: «монета/сеть: причина (сырые поля)».
    pub skipped: Vec<String>,
    /// Сети BSC с целым withdrawIntegerMultiple (смысл не подтверждён): «монета=значение».
    pub unclear_step: Vec<String>,
}

/// Что делает: разбор getall.
/// Принимает: ответ (массив монет), код сети BSC, площадку, «минимум депозита неизвестен — 0», справочник пар.
/// Возвращает: Parsed — сеть с неразборчивым полем (живой ответ 2026-11-02: withdrawMax не число) не валит весь
/// справочник, а только исключается.
/// Ошибки: BadResponse — ответ не массив или у монеты нет поля coin.
pub fn parse_networks(
    data: &Value,
    bsc: &str,
    venue: &VenueId,
    min_ok: bool,
    symbols: &Symbols,
) -> GatewayResult<Parsed> {
    let coins = data
        .as_array()
        .ok_or_else(|| errors::bad("config/getall не массив", &data.to_string()))?;
    let mut p = Parsed {
        list: Vec::new(),
        names: HashMap::new(),
        skipped: Vec::new(),
        unclear_step: Vec::new(),
    };
    for c in coins {
        let ticker = num::text(c, "coin")?;
        let Some(list) = c.get("networkList").and_then(Value::as_array) else {
            continue;
        };
        for n in list {
            let display = num::text_opt(n, "network").unwrap_or_default();
            let code = num::text_opt(n, "netWork").unwrap_or_else(|| display.clone());
            if code.is_empty() {
                continue;
            }
            p.names
                .insert((ticker.clone(), code.clone()), display.clone());
            let is_bsc = code == bsc;
            let contract = if is_bsc {
                num::text_opt(n, "contract").and_then(|a| a.parse().ok())
            } else {
                None
            };
            let (step, unclear) = withdraw_step(n);
            if is_bsc && let Some(v) = unclear {
                p.unclear_step.push(format!("{ticker}={v}"));
            }
            let info = || -> GatewayResult<CoinNetworkInfo> {
                Ok(CoinNetworkInfo {
                    venue: venue.clone(),
                    asset: Asset {
                        ticker: ticker.clone(),
                        bsc_contract: contract,
                    },
                    network: code.clone(),
                    deposit_enabled: flag(n, "depositEnable", &ticker)?,
                    withdraw_enabled: flag(n, "withdrawEnable", &ticker)?,
                    withdraw_fee: num::dec_opt(n, "withdrawFee")?,
                    withdraw_fee_pct: None,
                    withdraw_min: num::dec_opt(n, "withdrawMin")?,
                    withdraw_max: max_limit(n, "withdrawMax")?,
                    // Решение 107: в API MEXC минимального депозита нет. Пока Тигран не включил
                    // venue.deposit_min_absent_ok [mexc] — None, и ядро сделку на MEXC не делает (решение 35).
                    deposit_min: min_ok.then_some(Decimal::ZERO),
                    deposit_confirmations: num::text_opt(n, "minConfirm")
                        .and_then(|s| s.parse().ok()),
                    withdraw_precision: symbols.precision_of(&ticker),
                    withdraw_step: step,
                    updated_at: Utc::now(),
                })
            };
            match info() {
                Ok(i) => p.list.push(i),
                Err(e) => {
                    let raw = ["withdrawFee", "withdrawMin", "withdrawMax"]
                        .iter()
                        .map(|f| {
                            format!("{f}={}", n.get(*f).map_or("нет".into(), Value::to_string))
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    p.skipped
                        .push(format!("{ticker}/{code}: {} ({raw})", e.message));
                }
            }
        }
    }
    Ok(p)
}

/// Что делает: шаг суммы вывода из withdrawIntegerMultiple (см. шапку).
/// Принимает: сеть из networkList.
/// Возвращает: (шаг — только дробный 0 < v < 1; целое значение ≥ 1 — смысл не подтверждён). Нет поля, не число,
/// ноль — (None, None). Ошибок нет.
pub fn withdraw_step(n: &Value) -> (Option<Decimal>, Option<Decimal>) {
    match n.get("withdrawIntegerMultiple").and_then(num::to_dec) {
        Some(v) if v > Decimal::ZERO && v < Decimal::ONE => (Some(v), None),
        Some(v) if v >= Decimal::ONE => (None, Some(v)),
        _ => (None, None),
    }
}

/// Наибольшая сумма вывода. Живой ответ 2026-11-02: у 128 сетей withdrawMax = "99999999999999999999999999999999"
/// (32 девятки — больше предела Decimal ≈ 7.9e28): целое длиннее предела читается как Decimal::MAX — сумма заведомо
/// больше любой нашей. Иное нечисловое — ошибка (сеть пропускается). Ошибка BadResponse.
pub fn max_limit(n: &Value, field: &str) -> GatewayResult<Option<Decimal>> {
    match num::dec_opt(n, field) {
        Ok(v) => Ok(v),
        Err(e) => {
            let raw = n.get(field).and_then(Value::as_str).unwrap_or_default();
            let digits = raw.split('.').next().unwrap_or_default();
            if !digits.is_empty() && digits.len() > 28 && digits.bytes().all(|b| b.is_ascii_digit())
            {
                Ok(Some(Decimal::MAX))
            } else {
                Err(e)
            }
        }
    }
}

/// Флаг сети depositEnable / withdrawEnable — boolean. Иное — BadResponse (сеть пропускается).
pub fn flag(n: &Value, field: &str, ticker: &str) -> GatewayResult<bool> {
    match n.get(field) {
        Some(Value::Bool(b)) => Ok(*b),
        other => {
            let got = other.map_or("нет поля".to_string(), |v| format!("{v}"));
            Err(errors::bad(
                &format!("config/getall: {ticker}: {field} не boolean ({got})"),
                &n.to_string(),
            ))
        }
    }
}
