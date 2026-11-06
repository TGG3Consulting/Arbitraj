//! Файл: venues/gate/networks_parse.rs
//! Что делает: разбор справочника монет Gate (GET /spot/currencies — сети и флаги) вместе с комиссиями и пределами
//! вывода (GET /wallet/withdraw_status) в сети монет формата ядра (CoinNetworkInfo); точность вывода, минимальный
//! депозит и подтверждения — из уже полученных сведений одной монеты (Extra: /wallet/currency_chains, адрес депозита).
//! Кто вызывает: wallet.rs, tests/gate_real_data.rs.
//! От чего зависит: num.rs, errors.rs, core/types.
//! Настройки: нет (название сети BSC и «минимум депозита неизвестен — 0» передаёт wallet.rs).
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/ (SDK https://github.com/gate/gateapi-python):
//! - GET /spot/currencies (модель Currency, SpotCurrencyChain): currency, delisted, chains[] — name («Blockchain
//!   name»), addr («token address»), withdraw_disabled, withdraw_delayed, deposit_disabled. Поля верхнего уровня
//!   withdraw_disabled / deposit_disabled помечены «(deprecated)» — берутся флаги сети.
//! - GET /wallet/withdraw_status (модель WithdrawStatus): withdraw_fix_on_chains («Fixed withdrawal fee on multiple
//!   chains», словарь сеть → сумма), withdraw_percent_on_chains («Percentage withdrawal fee»), withdraw_amount_mini,
//!   withdraw_eachtime_limit. Решение 199: процент со знаком «%» («0.1%» — пример статьи Gate «Gate Withdrawal Fees by
//!   Coin and Network») — доля = число / 100 в CoinNetworkInfo.withdraw_fee_pct (полная комиссия = фикс + доля × сумма,
//!   допущение — из суммы). Ненулевое значение БЕЗ «%» — формат не описан: комиссия неизвестна (ядро по сети не
//!   торгует), монета и СЫРОЕ значение — в список «неясно» (журнал, вопрос Тиграну).
//! - GET /wallet/currency_chains?currency= (модель CurrencyChain): decimal («Withdrawal precision»), is_tag; адрес
//!   депозита (DepositAddress): min_deposit_amount, multichain_addresses[].min_confirms.

use std::collections::HashMap;

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, VenueId};
use crate::gateway::errors::GatewayResult;

use super::{errors, num};

/// Сведения одной монеты, которых нет в общих справочниках (кэш wallet.rs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extra {
    /// Сеть → знаков после запятой у суммы вывода (decimal /wallet/currency_chains; только целое).
    pub decimals: HashMap<String, u32>,
    /// Минимальный депозит монеты (min_deposit_amount адреса депозита).
    pub deposit_min: Option<Decimal>,
    /// Сеть → подтверждений для зачисления (min_confirms адреса депозита).
    pub confirms: HashMap<String, u32>,
}

/// Результат разбора.
pub struct Parsed {
    pub list: Vec<CoinNetworkInfo>,
    /// Монеты с процентной комиссией вывода не ноль в сети BSC — «монета=значение» (формат не подтверждён).
    pub unclear_fee: Vec<String>,
}

/// Что делает: сети формата ядра.
/// Принимает: ответ /spot/currencies (массив или одна монета), ответ /wallet/withdraw_status (массив), сеть BSC,
/// площадку, «минимум депозита неизвестен — 0», сведения монет (Extra), точность пары монеты (запасная).
/// Ошибки: BadResponse — ответ не массив или у монеты нет поля currency.
pub fn parse_networks(
    currencies: &Value,
    withdraw: &Value,
    bsc: &str,
    venue: &VenueId,
    min_ok: bool,
    extra: &HashMap<String, Extra>,
    pair_precision: &dyn Fn(&str) -> Option<u32>,
) -> GatewayResult<Parsed> {
    // Решение 131: без копии справочника — одна монета (GET /spot/currencies/{currency}) — срез из одного элемента.
    let coins: &[Value] = match currencies {
        Value::Array(a) => a,
        Value::Object(_) => std::slice::from_ref(currencies),
        _ => {
            return Err(errors::bad(
                "/spot/currencies не массив",
                &currencies.to_string(),
            ));
        }
    };
    let fees = by_currency(withdraw)?;
    let mut p = Parsed {
        list: Vec::new(),
        unclear_fee: Vec::new(),
    };
    for c in coins {
        let ticker = num::text(c, "currency")?;
        let delisted = matches!(c.get("delisted"), Some(Value::Bool(true)));
        let Some(chains) = c.get("chains").and_then(Value::as_array) else {
            continue;
        };
        let w = fees.get(ticker.as_str());
        let ex = extra.get(&ticker);
        for n in chains {
            let Some(chain) = num::text_opt(n, "name") else {
                continue;
            };
            let is_bsc = chain == bsc;
            let contract = is_bsc
                .then(|| num::text_opt(n, "addr").and_then(|a| a.parse().ok()))
                .flatten();
            let off = |f: &str| matches!(n.get(f), Some(Value::Bool(true)));
            let fee = w.and_then(|w| on_chain(w, "withdraw_fix_on_chains", &chain));
            let pct = w.map(|w| percent(w, &chain)).unwrap_or(Pct::None);
            let pct_unclear = matches!(pct, Pct::Unclear(_));
            if let (true, Pct::Unclear(raw)) = (is_bsc, &pct) {
                p.unclear_fee.push(format!("{ticker}={raw}"));
            }
            p.list.push(CoinNetworkInfo {
                venue: venue.clone(),
                asset: Asset {
                    ticker: ticker.clone(),
                    bsc_contract: contract,
                },
                network: chain.clone(),
                deposit_enabled: !delisted && !off("deposit_disabled"),
                withdraw_enabled: !delisted
                    && !off("withdraw_disabled")
                    && !off("withdraw_delayed"),
                withdraw_fee: if pct_unclear { None } else { fee },
                withdraw_fee_pct: match pct {
                    Pct::Share(v) => Some(v),
                    _ => None,
                },
                withdraw_min: w
                    .and_then(|w| num::dec_opt(w, "withdraw_amount_mini").ok().flatten()),
                withdraw_max: w
                    .and_then(|w| num::dec_opt(w, "withdraw_eachtime_limit").ok().flatten()),
                // Минимум депозита — из адреса депозита монеты, если он уже получен; иначе — решение 107.
                deposit_min: ex
                    .and_then(|e| e.deposit_min)
                    .or(min_ok.then_some(Decimal::ZERO)),
                deposit_confirmations: ex.and_then(|e| e.confirms.get(&chain).copied()),
                withdraw_precision: ex
                    .and_then(|e| e.decimals.get(&chain).copied())
                    .or_else(|| pair_precision(&ticker)),
                withdraw_step: None,
                updated_at: Utc::now(),
            });
        }
    }
    Ok(p)
}

/// Процентная комиссия сети (решение 199): нет / ноль — None; «X%» — доля X / 100; иное ненулевое — неясно (сырое).
#[derive(Debug, Clone, PartialEq)]
enum Pct {
    None,
    Share(Decimal),
    Unclear(String),
}

/// withdraw_percent_on_chains сети как есть. Принимает: запись withdraw_status, сеть. Ошибок нет: неразборчивое — Unclear.
fn percent(w: &Value, chain: &str) -> Pct {
    let Some(v) = w
        .get("withdraw_percent_on_chains")
        .and_then(|m| m.get(chain))
    else {
        return Pct::None;
    };
    let raw = v
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| v.to_string());
    let t = raw.trim();
    let (body, has_pct) = match t.strip_suffix('%') {
        Some(b) => (b.trim(), true),
        None => (t, false),
    };
    match (num::to_dec(&Value::String(body.to_string())), has_pct) {
        (Some(d), _) if d.is_zero() => Pct::None,
        (Some(d), true) if d > Decimal::ZERO => Pct::Share(d / Decimal::ONE_HUNDRED),
        _ => Pct::Unclear(raw),
    }
}

/// Записи withdraw_status по монете. Ошибка BadResponse — ответ не массив.
fn by_currency(withdraw: &Value) -> GatewayResult<HashMap<&str, &Value>> {
    let arr = withdraw
        .as_array()
        .ok_or_else(|| errors::bad("/wallet/withdraw_status не массив", &withdraw.to_string()))?;
    Ok(arr
        .iter()
        .filter_map(|w| w.get("currency").and_then(Value::as_str).map(|c| (c, w)))
        .collect())
}

/// Значение словаря «сеть → сумма» (строка или число). None — нет сети или не число. Ошибок нет.
fn on_chain(w: &Value, field: &str, chain: &str) -> Option<Decimal> {
    w.get(field)?.get(chain).and_then(|v| {
        let s = v
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string());
        num::to_dec(&Value::String(s.trim_end_matches('%').to_string()))
    })
}

/// Что делает: сведения монеты из /wallet/currency_chains (decimal — только целое ≥ 0).
/// Принимает: ответ (массив сетей), прежние сведения. Возвращает: обновлённые сведения и сети с is_tag = 1.
/// Ошибка BadResponse — ответ не массив.
pub fn chains_extra(data: &Value, mut ex: Extra) -> GatewayResult<(Extra, Vec<String>)> {
    let arr = data
        .as_array()
        .ok_or_else(|| errors::bad("/wallet/currency_chains не массив", &data.to_string()))?;
    let mut tagged = Vec::new();
    for c in arr {
        let Some(chain) = num::text_opt(c, "chain") else {
            continue;
        };
        if let Some(d) = num::text_opt(c, "decimal").and_then(|d| d.parse::<u32>().ok()) {
            ex.decimals.insert(chain.clone(), d);
        }
        if num::text_opt(c, "is_tag").as_deref() == Some("1") {
            tagged.push(chain);
        }
    }
    Ok((ex, tagged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Решение 199: «0.1%» — доля 0.001; «0%» и нет — нет; «10» без знака — неясно (сырое значение).
    #[test]
    fn percent_forms() {
        let w = json!({"withdraw_percent_on_chains": {"A": "0.1%", "B": "0%", "C": "10", "D": 10}});
        assert_eq!(percent(&w, "A"), Pct::Share("0.001".parse().unwrap()));
        assert_eq!(percent(&w, "B"), Pct::None);
        assert_eq!(percent(&w, "C"), Pct::Unclear("10".into()));
        assert_eq!(percent(&w, "D"), Pct::Unclear("10".into()));
        assert_eq!(percent(&w, "E"), Pct::None);
    }
}
