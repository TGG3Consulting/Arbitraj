//! Файл: venues/coinex/networks_parse.rs
//! Что делает: разбор справочника ввода/вывода CoinEx (GET /v2/assets/all-deposit-withdraw-config — массив, или
//! /v2/assets/deposit-withdraw-config?ccy= — одна монета) в сети монет формата ядра (CoinNetworkInfo): chain — название
//! сети ядра, адрес контракта у сети BSC (из contracts.rs), ввод/вывод, комиссия, минимумы вывода и депозита, точность
//! суммы вывода, подтверждения.
//! Кто вызывает: wallet.rs, tests/coinex_real_data.rs.
//! От чего зависит: num.rs, contracts.rs, core/types.
//! Настройки: нет (имя сети BSC и «минимум депозита неизвестен — 0» передаёт wallet.rs).
//!
//! Официально — Get Deposit and Withdrawal Configuration
//! https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/get-deposit-withdrawal-config (и list-all-…): asset.ccy,
//! asset.deposit_enabled / withdraw_enabled (монета целиком), chains[n]: chain, min_deposit_amount, min_withdraw_amount,
//! deposit_enabled, withdraw_enabled, deposit_delay_minutes, safe_confirmations, irreversible_confirmations,
//! deflation_rate «Deflation rate», withdrawal_fee, withdrawal_precision, memo, is_memo_required_for_deposit,
//! explorer_asset_url. Ввод (вывод) открыт, только если открыт и у монеты, и у сети.
//! Решения разбора (что в документации не определено — не угадывается):
//! - ненулевой deflation_rate у сети BSC — смысл одной строкой («Deflation rate»): комиссия вывода считается
//!   неизвестной (None — ядро не выводит по ней), монета — в журнал (вопрос Тиграну);
//! - min_deposit_amount (решение 371, Тигран 17.11): «0» — минимум 0 (документация не определяет); нет поля / null —
//!   по venue.deposit_min_absent_ok; нечитаемое — неизвестен (gateway/deposit_min.rs);
//! - подтверждения депозита — safe_confirmations («considered safe»; после скольких зачисляет CoinEx, не сказано);
//! - is_memo_required_for_deposit у сети BSC — в журнал (депозит с memo не поддерживается, deposit_addr.rs);
//! - ненулевой deposit_delay_minutes у сети BSC — в журнал (смысл не описан, вопрос Тиграну: удержание ли это вывода).

use std::collections::HashMap;

use alloy_primitives::Address;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, VenueId};
use crate::gateway::deposit_min as dmin;
use crate::gateway::errors::GatewayResult;

use super::num;

/// Результат разбора.
#[derive(Default)]
pub struct Parsed {
    pub list: Vec<CoinNetworkInfo>,
    /// Пропущенные сети: «монета/сеть: причина».
    pub skipped: Vec<String>,
    /// Сети BSC с memo для депозита: «монета».
    pub memo: Vec<String>,
    /// Сети BSC с ненулевым deflation_rate: «монета=доля» (комиссия вывода неизвестна).
    pub deflation: Vec<String>,
    /// Сети BSC с ненулевым deposit_delay_minutes: «монета=минуты».
    pub delay: Vec<String>,
    /// Сети BSC без адреса контракта в /assets/info: «монета».
    pub no_contract: Vec<String>,
}

/// Что делает: разбор data (массив монет или одна монета-объект) — только сеть BSC своя (как до решения 431).
/// Принимает: data, имя сети BSC (venue.bsc_network), адреса BSC (contracts.rs), площадку, «минимум депозита
/// неизвестен — 0». Возвращает: Parsed. Ошибки: как parse_with.
pub fn parse(
    data: &Value,
    bsc: &str,
    contracts: &HashMap<String, Address>,
    venue: &VenueId,
    min_ok: bool,
) -> GatewayResult<Parsed> {
    parse_with(data, &[(bsc, contracts)], venue, min_ok)
}

/// Что делает: разбор со своими сетями (решение 431). Принимает: data, свои сети — (имя сети в справочнике ввода/вывода,
/// адреса этой сети из /assets/info), первая — BSC (пометки журнала — только у неё), площадку, «минимум депозита
/// неизвестен — 0». Возвращает: Parsed. Ошибка BadResponse — data не массив и не объект, у монеты нет asset.ccy.
pub fn parse_with(
    data: &Value,
    own: &[(&str, &HashMap<String, Address>)],
    venue: &VenueId,
    min_ok: bool,
) -> GatewayResult<Parsed> {
    let coins: &[Value] = match data {
        Value::Array(a) => a,
        Value::Object(_) => std::slice::from_ref(data),
        _ => {
            return Err(super::errors::bad(
                "deposit-withdraw-config: data не массив",
                "",
            ));
        }
    };
    let mut p = Parsed::default();
    for c in coins {
        let asset = c.get("asset").unwrap_or(&Value::Null);
        let ticker = num::text(asset, "ccy")?;
        let coin_dep = num::flag(asset, "deposit_enabled").unwrap_or(false);
        let coin_wd = num::flag(asset, "withdraw_enabled").unwrap_or(false);
        let Some(chains) = c.get("chains").and_then(Value::as_array) else {
            continue;
        };
        for n in chains {
            let Some(chain) = num::text_opt(n, "chain") else {
                p.skipped.push(format!("{ticker}: сеть без chain"));
                continue;
            };
            let is_bsc = own.first().is_some_and(|(n, _)| *n == chain);
            let contract = own
                .iter()
                .find(|(n, _)| *n == chain)
                .and_then(|(_, m)| m.get(&ticker).copied());
            match network(
                n,
                &ticker,
                &chain,
                contract,
                venue,
                min_ok,
                (coin_dep, coin_wd),
            ) {
                Ok((info, deflation)) => {
                    if let Some(x) = dmin::note(MIN_FIELD, &min_raw(n)) {
                        p.skipped.push(format!("{ticker}/{chain}: {x}"));
                    }
                    if is_bsc {
                        note_bsc(&mut p, n, &ticker, contract.is_none(), deflation);
                    }
                    p.list.push(info);
                }
                Err(e) => p.skipped.push(format!("{ticker}/{chain}: {}", e.message)),
            }
        }
    }
    Ok(p)
}

/// Журнальные пометки сети BSC. Ошибок нет.
fn note_bsc(
    p: &mut Parsed,
    n: &Value,
    ticker: &str,
    no_contract: bool,
    deflation: Option<Decimal>,
) {
    if matches!(num::flag(n, "is_memo_required_for_deposit"), Ok(true)) {
        p.memo.push(ticker.to_string());
    }
    if let Some(x) = deflation {
        p.deflation.push(format!("{ticker}={x}"));
    }
    if let Ok(Some(m)) = num::dec_opt(n, "deposit_delay_minutes")
        && !m.is_zero()
    {
        p.delay.push(format!("{ticker}={m}"));
    }
    if no_contract {
        p.no_contract.push(ticker.to_string());
    }
}

/// Одна сеть. Возвращает: (сеть ядра, ненулевой deflation_rate). Ошибка BadResponse — поле не то.
fn network(
    n: &Value,
    ticker: &str,
    chain: &str,
    contract: Option<Address>,
    venue: &VenueId,
    min_ok: bool,
    (coin_dep, coin_wd): (bool, bool),
) -> GatewayResult<(CoinNetworkInfo, Option<Decimal>)> {
    let deflation = num::dec_opt(n, "deflation_rate")?.filter(|r| !r.is_zero());
    let fee = num::dec_opt(n, "withdrawal_fee")?;
    // Решение 371 (Тигран 17.11; документация смысл «0» не определяет): «0» — минимум 0; нет поля — по
    // venue.deposit_min_absent_ok; нечитаемое — неизвестен (gateway/deposit_min.rs).
    // Get Deposit and Withdrawal Configuration (https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-all-deposit-withdrawal-config): min_deposit_amount «Min. deposit amount», строка.
    let deposit_min = dmin::resolve(&min_raw(n), min_ok);
    let info = CoinNetworkInfo {
        venue: venue.clone(),
        asset: Asset {
            chain: None,
            ticker: ticker.to_string(),
            contract,
        },
        network: chain.to_string(),
        deposit_enabled: coin_dep && num::flag(n, "deposit_enabled")?,
        withdraw_enabled: coin_wd && num::flag(n, "withdraw_enabled")?,
        withdraw_fee: if deflation.is_some() { None } else { fee },
        use_net_withdrawal_budget: false,
        withdraw_fee_pct: None,
        withdraw_min: num::dec_opt(n, "min_withdraw_amount")?,
        withdraw_max: None,
        deposit_min,
        deposit_confirmations: num::text_opt(n, "safe_confirmations").and_then(|s| s.parse().ok()),
        withdraw_precision: num::text_opt(n, "withdrawal_precision")
            .and_then(|s| s.parse::<u32>().ok())
            .map(|p| p.min(28)),
        withdraw_step: None,
        updated_at: Utc::now(),
    };
    Ok((info, deflation))
}

/// Решение 371: имя поля минимума депозита сети (Get Deposit and Withdrawal Configuration, chains[n]).
const MIN_FIELD: &str = "min_deposit_amount";

/// Решение 371: вид значения минимума депозита сети (gateway/deposit_min.rs). Ошибок нет.
fn min_raw(n: &Value) -> dmin::Raw {
    dmin::classify(num::dec_opt(n, MIN_FIELD).map_err(|e| e.message))
}
