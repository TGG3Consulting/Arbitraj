//! Файл: venues/bitget/networks_parse.rs
//! Что делает: разбор справочника монет Bitget (GET /api/v2/spot/public/coins, весь или одна монета) в сети монет
//! формата ядра (CoinNetworkInfo): chain — название сети ядра, адрес контракта у сети BSC, ввод/вывод, комиссия,
//! минимумы вывода и депозита, точность и кратность суммы вывода, подтверждения, нужен ли tag.
//! Кто вызывает: wallet.rs, tests/bitget_real_data.rs.
//! От чего зависит: num.rs, errors.rs, core/types.
//! Настройки: нет (имя сети BSC и «минимум депозита неизвестен — 0» передаёт wallet.rs).
//!
//! Официально — Get Coin Info https://www.bitget.com/docs/catalog/classic-spot-market/classic-spot-market#get-coin-info:
//! coin, chains[]: chain «Chain name», needTag, withdrawable «Withdrawal supported», rechargeable «Deposit supported»
//! (строки "true"/"false"), withdrawFee «Withdrawal transaction fee», extraWithdrawFee «Extra charge. On chain
//! destruction: 0.1 means 10%», depositConfirm, withdrawConfirm, minDepositAmount, minWithdrawAmount, contractAddress
//! «coin contract address», withdrawStep «If the value is not 0, … withdrawal size should be multiple of the value»,
//! withdrawMinScale «Decimal places of withdrawal amount», congestion (normal / congested).
//!
//! Комиссия вывода: ненулевой extraWithdrawFee — доля от суммы, списывается ли она из суммы или сверху, не описано →
//! комиссия неизвестна (None: ядро не торгует с выводом по ней), такие сети BSC — в журнал (вопрос Тиграну, как KuCoin
//! withdrawFeeRate). Минимум депозита: значение «0» или пусто — смысл не описан → None, а при
//! venue.deposit_min_absent_ok = да — 0 (урок 21).

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
    /// Сети BSC, которым нужен tag (needTag = true): «монета» — депозит с tag не поддерживается.
    pub need_tag: Vec<String>,
    /// Пропущенные сети: «монета/сеть: причина».
    pub skipped: Vec<String>,
    /// Сети BSC с ненулевым extraWithdrawFee: «монета=доля» (комиссия неизвестна).
    pub extra_fee: Vec<String>,
    /// Сети BSC в перегрузке (congestion = congested): «монета».
    pub congested: Vec<String>,
}

/// Что делает: разбор массива монет (data Get Coin Info; при ?coin= — массив из одной монеты).
/// Принимает: data ответа, имя сети BSC (venue.bsc_network), площадку, «минимум депозита неизвестен — 0».
/// Возвращает: Parsed. Ошибка BadResponse — data не массив или у монеты нет поля coin.
pub fn parse(data: &Value, bsc: &str, venue: &VenueId, min_ok: bool) -> GatewayResult<Parsed> {
    let coins = data
        .as_array()
        .ok_or_else(|| super::errors::bad("coins: data не массив", ""))?;
    let mut p = Parsed::default();
    for c in coins {
        let ticker = num::text(c, "coin")?;
        let Some(chains) = c.get("chains").and_then(Value::as_array) else {
            continue;
        };
        for n in chains {
            let Some(chain) = num::text_opt(n, "chain") else {
                p.skipped.push(format!("{ticker}: сеть без chain"));
                continue;
            };
            let is_bsc = chain == bsc;
            match network(n, &ticker, &chain, is_bsc, venue, min_ok) {
                Ok((info, extra)) => {
                    if is_bsc && matches!(num::flag(n, "needTag"), Ok(true)) {
                        p.need_tag.push(ticker.clone());
                    }
                    if is_bsc && let Some(x) = extra {
                        p.extra_fee.push(format!("{ticker}={x}"));
                    }
                    if is_bsc && num::text_opt(n, "congestion").as_deref() == Some("congested") {
                        p.congested.push(ticker.clone());
                    }
                    p.list.push(info);
                }
                Err(e) => p.skipped.push(format!("{ticker}/{chain}: {}", e.message)),
            }
        }
    }
    Ok(p)
}

/// Одна сеть. Возвращает: (сеть ядра, ненулевой extraWithdrawFee, если есть). Ошибка BadResponse — поле не то.
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
    let extra = num::dec_opt(n, "extraWithdrawFee")?.filter(|r| !r.is_zero());
    let fee = num::dec_opt(n, "withdrawFee")?;
    let deposit_min = num::dec_opt(n, "minDepositAmount")?
        .filter(|m| !m.is_zero())
        .or(min_ok.then_some(Decimal::ZERO));
    let info = CoinNetworkInfo {
        venue: venue.clone(),
        asset: Asset {
            ticker: ticker.to_string(),
            bsc_contract: contract,
        },
        network: chain.to_string(),
        deposit_enabled: num::flag(n, "rechargeable")?,
        withdraw_enabled: num::flag(n, "withdrawable")?,
        withdraw_fee: if extra.is_some() { None } else { fee },
        withdraw_min: num::dec_opt(n, "minWithdrawAmount")?,
        withdraw_max: None,
        deposit_min,
        deposit_confirmations: num::text_opt(n, "depositConfirm").and_then(|s| s.parse().ok()),
        withdraw_precision: num::text_opt(n, "withdrawMinScale")
            .and_then(|s| s.parse::<u32>().ok())
            .map(|p| p.min(28)),
        withdraw_step: num::dec_opt(n, "withdrawStep")?.filter(|s| !s.is_zero()),
        updated_at: Utc::now(),
    };
    Ok((info, extra))
}
