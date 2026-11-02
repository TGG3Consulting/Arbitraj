//! Файл: venues/mexc/wallet.rs
//! Что делает: справочник монет и сетей MEXC, адрес депозита, стоимость вывода. Помнит соответствие кода сети
//! (netWork — для нового вывода) и отображаемого имени (network — для адреса депозита и истории).
//! Кто вызывает: mod.rs (функции контракта), transfers.rs (имя сети).
//! От чего зависит: rest.rs, num.rs, ctx.rs, symbols.rs (точность), core/types.
//! Настройки: venue.bsc_network [mexc] — код netWork сети BSC (тот, что принимает новый вывод; ядро сравнивает с ним
//! название сети монеты),
//! venue.deposit_min_absent_ok [mexc] (решение 107: минимального депозита в API MEXC нет).
//!
//! Официальная документация: GET /api/v3/capital/config/getall (SPOT_WITHDRAW_READ, вес 10)
//! https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/query-the-currency-information — networkList: network
//! («Withdraw network display name (legacy parameter, will be deprecated)»), netWork («Withdraw network (new parameter,
//! for the new withdraw endpoint)»), contract («Contract address»), depositEnable, withdrawEnable, withdrawFee,
//! withdrawMin, withdrawMax, minConfirm, withdrawIntegerMultiple («Withdraw amount increment» — смысл не ясен, не
//! используется). Минимального депозита в ответе нет.
//! GET /api/v3/capital/deposit/address?coin=&network= (вес 10)
//! https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/deposit-address-supporting-network — coin, network, address, memo.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::Utc;
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::symbols::Symbols;
use super::{num, rest};

/// Соответствие (монета, код сети) → отображаемое имя сети. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    names: Arc<RwLock<HashMap<(String, String), String>>>,
    /// Последний записанный в журнал список пропущенных сетей — пишем только при изменении (иначе запись на каждом
    /// обновлении справочника).
    skipped: Arc<RwLock<String>>,
}

impl Networks {
    /// Отображаемое имя сети монеты по коду. None — справочник ещё не получен или такой сети нет. Ошибок нет.
    pub fn display(&self, ticker: &str, code: &str) -> Option<String> {
        let m = self.names.read().unwrap_or_else(|p| p.into_inner());
        m.get(&(ticker.to_string(), code.to_string())).cloned()
    }
}

/// Справочник: все монеты MEXC и их сети. Название сети ядра — код netWork (нет его — отображаемое имя); адрес
/// контракта — только у сети BSC (venue.bsc_network совпал с кодом netWork). Точность вывода — знаки пары монеты
/// (baseAssetPrecision) и котируемой монеты (quoteAssetPrecision), решение 107. Ошибки: настройки, связь, формат.
pub async fn coin_networks(
    ctx: &Ctx,
    nets: &Networks,
    symbols: &Symbols,
) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    symbols.ensure_fresh(ctx).await?;
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v3/capital/config/getall",
        vec![],
        Op::Other,
    )
    .await?;
    let (list, names, skipped) = parse_networks(&data, &bsc, &ctx.venue(), min_ok, symbols)?;
    *nets.names.write().unwrap_or_else(|p| p.into_inner()) = names;
    let joined = skipped.join("; ");
    let changed = {
        let mut last = nets.skipped.write().unwrap_or_else(|p| p.into_inner());
        let c = *last != joined;
        *last = joined;
        c
    };
    if changed && !skipped.is_empty() {
        // Сеть с неразборчивым полем не попадает в справочник (по ней нет сделок), остальные — работают.
        let msg = format!(
            "config/getall: пропущено сетей с неразборчивыми полями: {} — {}",
            skipped.len(),
            skipped.join("; ")
        );
        let _ = ctx
            .journal
            .write(
                Level::Warn,
                "venue.networks",
                Some(super::ctx::ID),
                msg,
                None,
            )
            .await;
    }
    Ok(list)
}

/// Разбор getall. Принимает: ответ (массив монет), название сети BSC, площадку, «минимум депозита неизвестен — 0»,
/// справочник пар (точность). Возвращает: сети формата ядра, соответствие кодов именам и пропущенные сети («монета/сеть:
/// причина, сырое значение») — сеть с неразборчивым полем (живой ответ 2026-11-02: withdrawMax не число) не валит весь
/// справочник, а только исключается. Ошибки: BadResponse — ответ не массив или у монеты нет поля coin.
#[allow(clippy::type_complexity)]
pub fn parse_networks(
    data: &Value,
    bsc: &str,
    venue: &VenueId,
    min_ok: bool,
    symbols: &Symbols,
) -> GatewayResult<(
    Vec<CoinNetworkInfo>,
    HashMap<(String, String), String>,
    Vec<String>,
)> {
    let coins = data
        .as_array()
        .ok_or_else(|| errors::bad("config/getall не массив", &data.to_string()))?;
    let mut out = Vec::new();
    let mut names = HashMap::new();
    let mut skipped = Vec::new();
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
            names.insert((ticker.clone(), code.clone()), display.clone());
            let is_bsc = code == bsc;
            let contract = if is_bsc {
                num::text_opt(n, "contract").and_then(|a| a.parse().ok())
            } else {
                None
            };
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
                    withdraw_min: num::dec_opt(n, "withdrawMin")?,
                    withdraw_max: max_limit(n, "withdrawMax")?,
                    // Решение 107: в API MEXC минимального депозита нет. Пока Тигран не включил
                    // venue.deposit_min_absent_ok [mexc] — None, и ядро сделку на MEXC не делает (решение 35).
                    deposit_min: min_ok.then_some(Decimal::ZERO),
                    deposit_confirmations: num::text_opt(n, "minConfirm")
                        .and_then(|s| s.parse().ok()),
                    withdraw_precision: symbols.precision_of(&ticker),
                    updated_at: Utc::now(),
                })
            };
            match info() {
                Ok(i) => out.push(i),
                Err(e) => {
                    let raw = ["withdrawFee", "withdrawMin", "withdrawMax"]
                        .iter()
                        .map(|f| {
                            format!("{f}={}", n.get(*f).map_or("нет".into(), Value::to_string))
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    skipped.push(format!("{ticker}/{code}: {} ({raw})", e.message));
                }
            }
        }
    }
    Ok((out, names, skipped))
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

/// Флаг сети depositEnable / withdrawEnable — boolean. Иное — BadResponse (справочник целиком не обновляется).
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

/// Стоимость вывода: withdrawFee из getall (в монетах). Принимает: монету, сеть (код), сумму (не влияет).
/// Возвращает: [WithdrawFee]. Ошибки: монеты или сети нет, комиссия не сообщена, связь.
pub async fn withdraw_cost(
    ctx: &Ctx,
    nets: &Networks,
    symbols: &Symbols,
    asset: &Asset,
    network: &str,
) -> GatewayResult<Vec<Cost>> {
    let all = coin_networks(ctx, nets, symbols).await?;
    let info = all
        .iter()
        .find(|c| c.asset.ticker == asset.ticker && c.network == network)
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::NotConfigured,
                None,
                format!("У {} нет сети {network} в справочнике MEXC", asset.ticker),
            ))
        })?;
    let fee = info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "MEXC не сообщил комиссию вывода {} в сети {network}",
                asset.ticker
            ),
        ))
    })?;
    Ok(vec![Cost::new(
        CostKind::WithdrawFee,
        fee,
        asset.ticker.clone(),
        false,
    )])
}

/// Адрес депозита монеты в сети (код netWork): запрос по отображаемому имени сети, ответ — запись с этим именем или
/// кодом. Ошибки: сети нет у монеты (InvalidAddress), связь.
pub async fn deposit_address(
    ctx: &Ctx,
    nets: &Networks,
    symbols: &Symbols,
    asset: &Asset,
    network: &str,
) -> GatewayResult<String> {
    if nets.display(&asset.ticker, network).is_none() {
        coin_networks(ctx, nets, symbols).await?;
    }
    let display = nets
        .display(&asset.ticker, network)
        .unwrap_or_else(|| network.to_string());
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v3/capital/deposit/address",
        vec![("coin", asset.ticker.clone()), ("network", display.clone())],
        Op::Other,
    )
    .await?;
    let found = data.as_array().and_then(|a| {
        a.iter().find(|r| {
            let n = num::text_opt(r, "network");
            n.as_deref() == Some(display.as_str()) || n.as_deref() == Some(network)
        })
    });
    let found = found.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::InvalidAddress,
            None,
            format!(
                "У {} нет адреса депозита MEXC в сети {display} — создайте его в кабинете MEXC (Депозит)",
                asset.ticker
            ),
        ))
    })?;
    num::text(found, "address")
}
