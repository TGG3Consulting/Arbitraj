//! Файл: venues/kucoin/networks_parse.rs
//! Что делает: разбор справочника монет KuCoin (GET /api/v3/currencies и /api/v3/currencies/{currency}) в сети монет
//! формата ядра (CoinNetworkInfo): chainId — название сети ядра, адрес контракта у сети BSC, ввод/вывод, комиссия,
//! пределы, точность, подтверждения, нужен ли memo.
//! Кто вызывает: wallet.rs, tests/kucoin_real_data.rs.
//! От чего зависит: num.rs, errors.rs, core/types.
//! Настройки: нет (chainId сети BSC и «минимум депозита неизвестен — 0» передаёт wallet.rs).
//!
//! Официально — Get All Currencies https://www.kucoin.com/docs-new/api-3470152 и Get Currency
//! https://www.kucoin.com/docs-new/api-3470155: currency («A unique currency code that will never change»), chains[]:
//! chainName, chainId («Chain id of currency» — его принимают вывод и адрес депозита: «you can query the chainId through
//! the response of the GET /api/v3/currencies/{currency}»), withdrawalMinSize, depositMinSize, withdrawFeeRate,
//! withdrawalMinFee («Minimum fees charged for withdrawal»), isWithdrawEnabled, isDepositEnabled, confirms,
//! contractAddress, withdrawPrecision («maximum supported length after the decimal point of the withdrawal amount»),
//! maxWithdraw, needTag («Need for memo/tag or not»).
//!
//! Комиссия вывода: формулы «минимальная комиссия + доля от суммы» в документации нет (страница Get All Currencies —
//! только пример со значением "0", описаний полей нет; проверено 06.11.2026, решение 199). Сеть с withdrawFeeRate = 0 —
//! комиссия = withdrawalMinFee; с ненулевой долей — комиссия неизвестна (None: ядро не торгует с выводом по ней), такие
//! сети BSC — в журнал с СЫРЫМ значением (для разбора формата по живому ответу) и вопрос Тиграну (docs/TASKS.md).

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
    /// Сети BSC, которым нужен memo: «монета» (адрес депозита с memo не поддерживается).
    pub need_tag: Vec<String>,
    /// Пропущенные сети: «монета/сеть: причина».
    pub skipped: Vec<String>,
    /// Сети BSC с ненулевой долей комиссии вывода: «монета=доля» (комиссия неизвестна).
    pub fee_rate: Vec<String>,
}

/// Что делает: разбор массива монет (Get All Currencies) или одной монеты (Get Currency — объект).
/// Принимает: data ответа, chainId сети BSC, площадку, «минимум депозита неизвестен — 0».
/// Возвращает: Parsed. Ошибка BadResponse — data не массив и не объект с currency.
pub fn parse(data: &Value, bsc: &str, venue: &VenueId, min_ok: bool) -> GatewayResult<Parsed> {
    let one = std::slice::from_ref(data);
    let coins: &[Value] = match data {
        Value::Array(a) => a,
        Value::Object(_) => one,
        _ => return Err(super::errors::bad("currencies: data не массив", "")),
    };
    let mut p = Parsed::default();
    for c in coins {
        let ticker = num::text(c, "currency")?;
        let Some(chains) = c.get("chains").and_then(Value::as_array) else {
            continue;
        };
        for n in chains {
            let Some(chain) = num::text_opt(n, "chainId") else {
                p.skipped.push(format!("{ticker}: сеть без chainId"));
                continue;
            };
            let is_bsc = chain == bsc;
            match network(n, &ticker, &chain, is_bsc, venue, min_ok) {
                Ok((info, rate)) => {
                    if is_bsc && matches!(n.get("needTag"), Some(Value::Bool(true))) {
                        p.need_tag.push(ticker.clone());
                    }
                    if is_bsc && rate.is_some() {
                        // Решение 199: сырое значение как пришло — формат и формула в документации не описаны.
                        let raw = n
                            .get("withdrawFeeRate")
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        p.fee_rate.push(format!("{ticker}={raw}"));
                    }
                    p.list.push(info);
                }
                Err(e) => p.skipped.push(format!("{ticker}/{chain}: {}", e.message)),
            }
        }
    }
    Ok(p)
}

/// Одна сеть. Возвращает: (сеть ядра, ненулевая доля комиссии вывода, если есть). Ошибка BadResponse — поле не то.
fn network(
    n: &Value,
    ticker: &str,
    chain: &str,
    is_bsc: bool,
    venue: &VenueId,
    min_ok: bool,
) -> GatewayResult<(CoinNetworkInfo, Option<Decimal>)> {
    let contract = if is_bsc {
        num::text_opt(n, "contractAddress").and_then(|a| a.parse().ok())
    } else {
        None
    };
    let rate = num::dec_opt(n, "withdrawFeeRate")?.filter(|r| !r.is_zero());
    let min_fee = num::dec_opt(n, "withdrawalMinFee")?;
    let deposit_min = num::dec_opt(n, "depositMinSize")?.or(min_ok.then_some(Decimal::ZERO));
    let info = CoinNetworkInfo {
        venue: venue.clone(),
        asset: Asset {
            ticker: ticker.to_string(),
            bsc_contract: contract,
        },
        network: chain.to_string(),
        deposit_enabled: num::flag(n, "isDepositEnabled")?,
        withdraw_enabled: num::flag(n, "isWithdrawEnabled")?,
        withdraw_fee: if rate.is_some() { None } else { min_fee },
        withdraw_fee_pct: None,
        withdraw_min: num::dec_opt(n, "withdrawalMinSize")?,
        withdraw_max: num::dec_opt(n, "maxWithdraw")?,
        deposit_min,
        deposit_confirmations: num::text_opt(n, "confirms").and_then(|s| s.parse().ok()),
        withdraw_precision: num::text_opt(n, "withdrawPrecision")
            .and_then(|s| s.parse::<u32>().ok())
            .map(|p| p.min(28)),
        withdraw_step: None,
        updated_at: Utc::now(),
    };
    Ok((info, rate))
}
