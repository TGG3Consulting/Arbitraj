//! Файл: venues/phemex/networks_parse.rs
//! Что делает: сети одной монеты Phemex в формате ядра (CoinNetworkInfo) — сведение двух справочников по chainName:
//! настройки депозита (статус ввода, минимум депозита, подтверждения, адрес контракта) и настройки вывода (статус
//! вывода, комиссия, минимум, адрес контракта).
//! Кто вызывает: wallet.rs, tests/phemex_real_data.rs.
//! От чего зависит: num.rs, core/types.
//! Настройки: нет (имя сети BSC и «минимум депозита неизвестен — 0» передаёт wallet.rs).
//!
//! Официально (Deposit And Withdraw REST API, https://phemex-docs.github.io/):
//! - Query deposit chain settings GET /phemex-deposit/wallets/api/chainCfg?currency= (#query-deposit-chain-settings) —
//!   data: массив {currency, minAmountRv «minimal deposit amount», confirmations, chainName, status «Active or Suspend.
//!   Active mean available for deposit», contractAddress}.
//! - Query withdraw chain settings GET /phemex-withdraw/wallets/api/asset/info?currency= (#query-withdraw-chain-settings)
//!   — data {currency, chainInfos: [{chainName, status «Active mean Withdrawable», contractAddress, minWithdrawAmountRv,
//!   minWithdrawAmountWithFeeRv, withdrawFeeRv «withdraw fee required for user input withdraw amount»,
//!   receiveAmountRv}]}.
//!
//! Адрес контракта сети BSC — из обоих; разные — сеть пропускается (не угадываем, какой верный). Минимум депозита «0»
//! или пусто — смысл не описан → None, а при venue.deposit_min_absent_ok = да — 0 (урок 21). Точность и шаг суммы
//! вывода в документации не описаны — None (ядро не округляет по ним; проверить малым выводом).

use std::collections::BTreeMap;

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, VenueId};
use crate::gateway::errors::GatewayResult;

use super::num;

/// Итог сведения: сети монеты и пропущенные «монета/сеть: причина».
#[derive(Default)]
pub struct Parsed {
    pub list: Vec<CoinNetworkInfo>,
    pub skipped: Vec<String>,
}

/// Состояние «Active» статусов ввода и вывода (Active / Suspend).
const ACTIVE: &str = "Active";

/// Что делает: сводит справочники одной монеты.
/// Принимает: тикер, data chainCfg (массив), data asset/info (объект), имя сети BSC, площадку, «минимум 0».
/// Возвращает: Parsed. Ошибка BadResponse — data chainCfg не массив или у asset/info нет chainInfos.
pub fn merge(
    ticker: &str,
    deposit: &Value,
    withdraw: &Value,
    bsc: &str,
    venue: &VenueId,
    min_ok: bool,
) -> GatewayResult<Parsed> {
    let dep = deposit
        .as_array()
        .ok_or_else(|| super::errors::bad("chainCfg: data не массив", &deposit.to_string()))?;
    let wd = num::arr(withdraw, "chainInfos")?;
    let mut chains: BTreeMap<String, (Option<&Value>, Option<&Value>)> = BTreeMap::new();
    for d in dep {
        if let Some(c) = num::text_opt(d, "chainName") {
            chains.entry(c).or_default().0 = Some(d);
        }
    }
    for w in wd {
        if let Some(c) = num::text_opt(w, "chainName") {
            chains.entry(c).or_default().1 = Some(w);
        }
    }
    let mut p = Parsed::default();
    for (chain, (d, w)) in chains {
        match network(ticker, &chain, d, w, chain == bsc, venue, min_ok) {
            Ok(info) => p.list.push(info),
            Err(why) => p.skipped.push(format!("{ticker}/{chain}: {why}")),
        }
    }
    Ok(p)
}

/// Одна сеть из записи депозита и (или) вывода. Ошибка — текст причины (поле не число, адреса контракта разные).
fn network(
    ticker: &str,
    chain: &str,
    d: Option<&Value>,
    w: Option<&Value>,
    is_bsc: bool,
    venue: &VenueId,
    min_ok: bool,
) -> Result<CoinNetworkInfo, String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let addr = |v: Option<&Value>| v.and_then(|v| num::text_opt(v, "contractAddress"));
    let contract = if is_bsc {
        match (addr(d), addr(w)) {
            (Some(a), Some(b)) if !a.eq_ignore_ascii_case(&b) => {
                return Err(format!("адрес контракта у ввода {a} и у вывода {b} разный"));
            }
            (a, b) => a.or(b).and_then(|a| a.parse().ok()),
        }
    } else {
        None
    };
    let active =
        |v: Option<&Value>| v.and_then(|v| num::text_opt(v, "status")).as_deref() == Some(ACTIVE);
    let dec = |v: Option<&Value>, f: &str| match v {
        Some(v) => num::dec_opt(v, f).map_err(m),
        None => Ok(None),
    };
    let deposit_min = dec(d, "minAmountRv")?
        .filter(|x| !x.is_zero())
        .or(min_ok.then_some(Decimal::ZERO));
    Ok(CoinNetworkInfo {
        venue: venue.clone(),
        asset: Asset {
            ticker: ticker.to_string(),
            bsc_contract: contract,
        },
        network: chain.to_string(),
        deposit_enabled: active(d),
        withdraw_enabled: active(w),
        withdraw_fee: dec(w, "withdrawFeeRv")?,
        withdraw_min: dec(w, "minWithdrawAmountRv")?,
        withdraw_max: None,
        deposit_min,
        deposit_confirmations: d
            .and_then(|v| v.get("confirmations"))
            .and_then(num::to_int)
            .and_then(|c| u32::try_from(c).ok()),
        withdraw_precision: None,
        withdraw_step: None,
        updated_at: Utc::now(),
    })
}
