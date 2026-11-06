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
//! - min_deposit_amount «0» или пусто — смысл не описан → None, а при venue.deposit_min_absent_ok = да — 0 (урок 21);
//! - подтверждения депозита — safe_confirmations («considered safe»; после скольких зачисляет CoinEx, не сказано);
//! - is_memo_required_for_deposit у сети BSC — в журнал (депозит с memo не поддерживается, deposit_addr.rs);
//! - ненулевой deposit_delay_minutes у сети BSC — в журнал (смысл не описан, вопрос Тиграну: удержание ли это вывода).

use std::collections::HashMap;

use alloy_primitives::Address;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, VenueId};
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

/// Что делает: разбор data (массив монет или одна монета-объект).
/// Принимает: data, имя сети BSC (venue.bsc_network), адреса BSC (contracts.rs), площадку, «минимум депозита
/// неизвестен — 0». Возвращает: Parsed. Ошибка BadResponse — data не массив и не объект, у монеты нет asset.ccy.
pub fn parse(
    data: &Value,
    bsc: &str,
    contracts: &HashMap<String, Address>,
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
            let is_bsc = chain == bsc;
            let contract = if is_bsc {
                contracts.get(&ticker).copied()
            } else {
                None
            };
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
    let deposit_min = num::dec_opt(n, "min_deposit_amount")?
        .filter(|m| !m.is_zero())
        .or(min_ok.then_some(Decimal::ZERO));
    let info = CoinNetworkInfo {
        venue: venue.clone(),
        asset: Asset {
            ticker: ticker.to_string(),
            bsc_contract: contract,
        },
        network: chain.to_string(),
        deposit_enabled: coin_dep && num::flag(n, "deposit_enabled")?,
        withdraw_enabled: coin_wd && num::flag(n, "withdraw_enabled")?,
        withdraw_fee: if deflation.is_some() { None } else { fee },
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
