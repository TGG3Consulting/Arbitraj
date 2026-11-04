//! Файл: venues/okx/networks_parse.rs
//! Что делает: разбор справочника монет OKX (GET /api/v5/asset/currencies) в сети монет формата ядра (CoinNetworkInfo):
//! название сети ядра (chain без приставки «<монета>-»), адрес контракта у сети BSC, ввод/вывод, комиссия, пределы,
//! точность, подтверждения, нужен ли memo — и соответствие «сеть ядра → chain OKX» для вывода и адреса депозита.
//! Кто вызывает: wallet.rs, tests/okx_real_data.rs.
//! От чего зависит: num.rs, errors.rs, core/types.
//! Настройки: нет (сеть BSC и «минимум депозита неизвестен — 0» передаёт wallet.rs).
//!
//! Официально — «Funding Account → REST API → Get currencies» https://www.okx.com/docs-v5/en/: ccy, chain («Chain name,
//! e.g. USDT-ERC20, USDT-TRC20» — в имени сети есть тикер монеты, поэтому у ядра сеть — часть после «<ccy>-», одна на все
//! монеты: venue.bsc_network, например «BSC» — проверить на живом ответе), canDep / canWd («The availability to deposit
//! from chain» / «to withdraw to chain»), minDep («The minimum deposit amount of currency in a single transaction»), minWd,
//! maxWd («… on-chain withdrawal … in a single transaction»), wdTickSz («The withdrawal precision, indicating the number
//! of digits after the decimal point»), fee («The fixed withdrawal fee Apply to on-chain withdrawal»), ctAddr («Contract
//! address»; в ответе адреса депозита — только «Last 6 digits», здесь — полный: проверить на живом ответе), needTag
//! («Whether tag/memo information is required for withdrawal»), minDepArrivalConfirm («The minimum number of blockchain
//! confirmations to acknowledge fund deposit»), minWdUnlockConfirm («… required for withdrawal of a deposit»).

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
    /// (монета, сеть ядра, chain OKX) — для вывода и адреса депозита (обратно к имени OKX без догадок).
    pub chains: Vec<(String, String, String)>,
    /// Сети BSC, которым нужен memo: «монета» (вывод с memo и адрес депозита с memo не поддерживаются).
    pub need_tag: Vec<String>,
    /// Сети BSC с непустым ctAddr, который не адрес 0x…40 hex: «монета=значение» (монета не сопоставляется).
    pub bad_contract: Vec<String>,
    /// Пропущенные сети: «монета/сеть: причина».
    pub skipped: Vec<String>,
}

/// Сеть ядра из chain OKX: часть после «<ccy>-», если chain так начинается; иначе chain как есть. Ошибок нет.
pub fn core_network(ccy: &str, chain: &str) -> String {
    chain
        .strip_prefix(ccy)
        .and_then(|r| r.strip_prefix('-'))
        .filter(|r| !r.is_empty())
        .unwrap_or(chain)
        .to_string()
}

/// Что делает: разбор массива data ответа Get currencies (строка = монета + сеть).
/// Принимает: data, сеть BSC ядра (venue.bsc_network), площадку, «минимум депозита неизвестен — 0».
/// Возвращает: Parsed. Ошибка BadResponse — data не массив.
pub fn parse(data: &Value, bsc: &str, venue: &VenueId, min_ok: bool) -> GatewayResult<Parsed> {
    let rows = data
        .as_array()
        .ok_or_else(|| super::errors::bad("currencies: data не массив", ""))?;
    let mut p = Parsed::default();
    for r in rows {
        let Some(ticker) = num::text_opt(r, "ccy") else {
            p.skipped.push("строка без ccy".into());
            continue;
        };
        let Some(chain) = num::text_opt(r, "chain") else {
            p.skipped.push(format!("{ticker}: сеть без chain"));
            continue;
        };
        let net = core_network(&ticker, &chain);
        let is_bsc = net == bsc;
        match network(r, &ticker, &net, is_bsc, venue, min_ok) {
            Ok((info, bad)) => {
                if is_bsc && matches!(r.get("needTag"), Some(Value::Bool(true))) {
                    p.need_tag.push(ticker.clone());
                }
                if let Some(b) = bad {
                    p.bad_contract.push(format!("{ticker}={b}"));
                }
                p.chains.push((ticker.clone(), net, chain));
                p.list.push(info);
            }
            Err(e) => p.skipped.push(format!("{ticker}/{chain}: {}", e.message)),
        }
    }
    Ok(p)
}

/// Одна сеть. Возвращает: (сеть ядра; неразборчивый ctAddr сети BSC, если есть). Ошибка BadResponse — поле не то.
fn network(
    r: &Value,
    ticker: &str,
    net: &str,
    is_bsc: bool,
    venue: &VenueId,
    min_ok: bool,
) -> GatewayResult<(CoinNetworkInfo, Option<String>)> {
    let raw = is_bsc.then(|| num::text_opt(r, "ctAddr")).flatten();
    let contract = raw.as_deref().and_then(|a| a.parse().ok());
    let bad = raw.filter(|_| contract.is_none());
    let deposit_min = num::dec_opt(r, "minDep")?.or(min_ok.then_some(Decimal::ZERO));
    let info = CoinNetworkInfo {
        venue: venue.clone(),
        asset: Asset {
            ticker: ticker.to_string(),
            bsc_contract: contract,
        },
        network: net.to_string(),
        deposit_enabled: num::flag(r, "canDep")?,
        withdraw_enabled: num::flag(r, "canWd")?,
        withdraw_fee: num::dec_opt(r, "fee")?,
        withdraw_min: num::dec_opt(r, "minWd")?,
        withdraw_max: num::dec_opt(r, "maxWd")?,
        deposit_min,
        deposit_confirmations: num::text_opt(r, "minDepArrivalConfirm")
            .and_then(|s| s.parse().ok()),
        withdraw_precision: num::text_opt(r, "wdTickSz")
            .and_then(|s| s.parse::<u32>().ok())
            .map(|p| p.min(28)),
        withdraw_step: None,
        updated_at: Utc::now(),
    };
    Ok((info, bad))
}
