//! Файл: venues/htx/networks_parse.rs
//! Что делает: разбор справочника монет и сетей HTX (GET /v2/reference/currencies — весь или одна монета) в сети
//! формата ядра (CoinNetworkInfo): название сети ядра, ввод/вывод, комиссия, пределы, точность, подтверждения, минимум
//! депозита — и соответствие «сеть ядра → chain HTX» для вывода и адреса депозита; адрес контракта — из отдельного
//! справочника сетей (GET /v1/settings/common/chains), если Тигран указал его поле (venue.contract_field).
//! Кто вызывает: wallet.rs, tests/htx_real_data.rs.
//! От чего зависит: num.rs, ctx.rs (тикер), core/types.
//! Настройки: нет (сеть BSC, поле адреса контракта и «минимум депозита неизвестен — 0» передаёт wallet.rs).
//!
//! Официально — APIv2 Currency & Chains https://huobiapi.github.io/docs/spot/v1/en/#apiv2-currency-amp-chains:
//! currency, instStatus (normal, delisted), chains[]: chain «chain name» (своё у монеты: «trc20usdt»), displayName,
//! baseChain, baseChainProtocol (примеры ERC20, TRC20, OMNI), numOfConfirmations, numOfFastConfirmations («trading
//! allowed, withdrawal disallowed» — см. deposits.rs, withdraw_hold), minDepositAmt, depositStatus / withdrawStatus
//! (allowed, prohibited), minWithdrawAmt, maxWithdrawAmt, withdrawPrecision, withdrawFeeType (fixed, circulated, ratio),
//! transactFeeWithdraw («only for fixed»), min/maxTransactFeeWithdraw («only for circulated or ratio»),
//! transactFeeRateWithdraw («only for ratio»). Имя chain своё у каждой монеты, поэтому сеть ядра — baseChainProtocol
//! (одна на все монеты: venue.bsc_network, например «BEP20» — проверить на живом ответе CAKE), без него — chain.
//! Комиссия вывода — только fixed (transactFeeWithdraw); circulated / ratio — как HTX считает, не описано → комиссия
//! неизвестна (None: ядро не выводит), такие сети BSC — в журнал (вопрос Тиграну).
//! Адреса контракта токена в этом ответе НЕТ. В Get Chains Information есть ac «address of chain» (смысл не ясен), в
//! официальном SDK (pkg/model/common/getchainsresponse.go) — ещё ca без описания. Без venue.contract_field адрес не
//! берётся: монеты HTX с DEX и другими биржами не сопоставляются (правило CLAUDE.md, урок 8) — не угадываем.

use std::collections::HashMap;

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, VenueId};
use crate::gateway::errors::GatewayResult;

use super::ctx::ticker;
use super::num;

/// Адреса контракта: (тикер ядра, chain HTX) → адрес (строкой, как в ответе).
pub type Contracts = HashMap<(String, String), String>;

/// Результат разбора.
#[derive(Default)]
pub struct Parsed {
    pub list: Vec<CoinNetworkInfo>,
    /// (тикер, сеть ядра, chain HTX) — для вывода и адреса депозита.
    pub chains: Vec<(String, String, String)>,
    /// Пропущенные сети: «монета/сеть: причина».
    pub skipped: Vec<String>,
    /// Сети BSC с комиссией вывода не fixed: «монета=тип» (комиссия неизвестна).
    pub fee_unknown: Vec<String>,
    /// Монеты, у которых сеть BSC встречается больше одного раза (неоднозначно — сеть BSC не берётся).
    pub ambiguous: Vec<String>,
}

/// Сеть ядра из сети HTX: baseChainProtocol, если есть, иначе chain. Ошибок нет.
pub fn core_network(n: &Value) -> Option<String> {
    num::text_opt(n, "baseChainProtocol").or_else(|| num::text_opt(n, "chain"))
}

/// Что делает: разбор data /v2/reference/currencies.
/// Принимает: data, сеть BSC ядра, адреса контракта (пусто — поле не задано), площадку, «минимум депозита неизвестен
/// — 0». Возвращает: Parsed. Ошибка BadResponse — data не массив или у монеты нет currency.
pub fn parse(
    data: &Value,
    bsc: &str,
    contracts: &Contracts,
    venue: &VenueId,
    min_ok: bool,
) -> GatewayResult<Parsed> {
    let coins = match data {
        Value::Null => return Ok(Parsed::default()),
        Value::Array(a) => a,
        _ => return Err(super::errors::bad("currencies: data не массив", "")),
    };
    let mut p = Parsed::default();
    for c in coins {
        let cur = num::text(c, "currency")?;
        let t = ticker(&cur);
        let delisted = num::text_opt(c, "instStatus").as_deref() == Some("delisted");
        let Some(chains) = c.get("chains").and_then(Value::as_array) else {
            continue;
        };
        let bsc_count = chains
            .iter()
            .filter(|n| core_network(n).as_deref() == Some(bsc))
            .count();
        if bsc_count > 1 {
            p.ambiguous.push(t.clone());
        }
        for n in chains {
            let (Some(chain), Some(net)) = (num::text_opt(n, "chain"), core_network(n)) else {
                p.skipped.push(format!("{t}: сеть без chain"));
                continue;
            };
            let is_bsc = net == bsc;
            if is_bsc && bsc_count > 1 {
                continue;
            }
            let contract = is_bsc
                .then(|| contracts.get(&(t.clone(), chain.clone())))
                .flatten()
                .and_then(|a| a.parse().ok());
            match network(n, &t, &net, contract, delisted, venue, min_ok) {
                Ok(info) => {
                    if is_bsc && info.withdraw_fee.is_none() {
                        let kind = num::text_opt(n, "withdrawFeeType").unwrap_or_default();
                        p.fee_unknown.push(format!("{t}={kind}"));
                    }
                    p.chains.push((t.clone(), net, chain));
                    p.list.push(info);
                }
                Err(e) => p.skipped.push(format!("{t}/{chain}: {}", e.message)),
            }
        }
    }
    Ok(p)
}

/// Одна сеть. Возвращает: сеть ядра. Ошибка BadResponse — поле не того вида.
fn network(
    n: &Value,
    t: &str,
    net: &str,
    contract: Option<alloy_primitives::Address>,
    delisted: bool,
    venue: &VenueId,
    min_ok: bool,
) -> GatewayResult<CoinNetworkInfo> {
    let fixed = num::text_opt(n, "withdrawFeeType").as_deref() == Some("fixed");
    let fee = if fixed {
        num::dec_opt(n, "transactFeeWithdraw")?
    } else {
        None
    };
    let allowed = |f: &str| !delisted && num::text_opt(n, f).as_deref() == Some("allowed");
    let deposit_min = num::dec_opt(n, "minDepositAmt")?
        .filter(|m| !m.is_zero())
        .or(min_ok.then_some(Decimal::ZERO));
    Ok(CoinNetworkInfo {
        venue: venue.clone(),
        asset: Asset {
            ticker: t.to_string(),
            bsc_contract: contract,
        },
        network: net.to_string(),
        deposit_enabled: allowed("depositStatus"),
        withdraw_enabled: allowed("withdrawStatus"),
        withdraw_fee: fee,
        withdraw_min: num::dec_opt(n, "minWithdrawAmt")?,
        withdraw_max: num::dec_opt(n, "maxWithdrawAmt")?,
        deposit_min,
        deposit_confirmations: num::text_opt(n, "numOfConfirmations").and_then(|s| s.parse().ok()),
        withdraw_precision: num::text_opt(n, "withdrawPrecision")
            .and_then(|s| s.parse::<u32>().ok())
            .map(|p| p.min(28)),
        withdraw_step: None,
        updated_at: Utc::now(),
    })
}

/// Что делает: разбор data /v1/settings/common/chains в адреса контракта по полю field («ac» или «ca»).
/// Принимает: data, имя поля. Возвращает: (тикер, chain) → значение поля (пустое — пропускается). Ошибка BadResponse —
/// data не массив.
pub fn contracts(data: &Value, field: &str) -> GatewayResult<Contracts> {
    let list = match data {
        Value::Null => return Ok(Contracts::new()),
        Value::Array(a) => a,
        _ => return Err(super::errors::bad("chains: data не массив", "")),
    };
    let mut out = Contracts::new();
    for c in list {
        let (Some(cur), Some(chain), Some(addr)) = (
            num::text_opt(c, "currency"),
            num::text_opt(c, "chain"),
            num::text_opt(c, field),
        ) else {
            continue;
        };
        out.insert((ticker(&cur), chain), addr);
    }
    Ok(out)
}
