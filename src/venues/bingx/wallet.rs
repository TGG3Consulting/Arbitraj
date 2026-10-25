//! Файл: venues/bingx/wallet.rs
//! Что делает: справочник монет и сетей BingX и адрес депозита.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, core/types.
//! Настройки: venue.bsc_network.
//!
//! Официальная документация (docs-v3 и api-ai-skills/spot-wallet):
//! - GET /openApi/wallets/v1/capital/config/getall — networkList: network, depositEnable, withdrawEnable,
//!   withdrawFee, withdrawMin, withdrawMax, minConfirm, depositMin (только в официальном примере ответа,
//!   в таблице полей его нет), contractAddress (поле есть в api-ai-skills от
//!   2026-10-22; в примере docs-v3 его нет — проверяется на реальном ответе).
//! - GET /openApi/wallets/v1/capital/deposit/address — data.data[]: coin, network, address, tag, status.

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use rust_decimal::Decimal;

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, rest};

/// Справочник: все монеты BingX и их сети. Адрес контракта ставится, если сеть — BSC (venue.bsc_network).
pub async fn coin_networks(ctx: &Ctx) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let bsc = ctx.text("venue.bsc_network")?;
    let data = rest::signed(
        ctx,
        Method::GET,
        "/openApi/wallets/v1/capital/config/getall",
        vec![],
        Op::Other,
    )
    .await?;
    parse_networks(&data, &bsc, &ctx.venue())
}

/// Разбор ответа config/getall (data — массив монет с networkList). Адрес контракта ставится только сети с названием
/// bsc (venue.bsc_network); пустой или неразборчивый адрес (у BNB в BEP20 — "") — None: ядро покажет «сеть BSC есть,
/// адреса контракта нет». Принимает: data ответа, название сети BSC, площадку. Ошибки: BadResponse — формат.
pub fn parse_networks(
    data: &Value,
    bsc: &str,
    venue: &VenueId,
) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let coins = data
        .as_array()
        .ok_or_else(|| errors::bad("data не массив", &data.to_string()))?;
    let mut out = Vec::new();
    for c in coins {
        let ticker = num::text(c, "coin")?;
        for n in num::arr(c, "networkList")? {
            let network = num::text(n, "network")?;
            let contract = if network == bsc {
                num::text_opt(n, "contractAddress").and_then(|a| a.parse().ok())
            } else {
                None
            };
            out.push(CoinNetworkInfo {
                venue: venue.clone(),
                asset: Asset {
                    ticker: ticker.clone(),
                    bsc_contract: contract,
                },
                network,
                deposit_enabled: flag(n, "depositEnable", &ticker)?,
                withdraw_enabled: flag(n, "withdrawEnable", &ticker)?,
                withdraw_fee: num::dec_opt(n, "withdrawFee")?,
                withdraw_min: num::dec_opt(n, "withdrawMin")?,
                withdraw_max: num::dec_opt(n, "withdrawMax")?,
                // depositMin есть в официальном примере ответа config/getall (docs-v3), но его нет
                // в таблице полей ответа. Нет поля в реальном ответе — None, и ядро по такой
                // монете сделку не делает (решение 35: минимальный депозит неизвестен).
                deposit_min: num::dec_opt(n, "depositMin")?,
                deposit_confirmations: num::text_opt(n, "minConfirm").and_then(|s| s.parse().ok()),
                // Поле есть в живом ответе (tests/data/bingx_2026-10-24/config_getall.json), в docs-v3 его нет.
                withdraw_precision: n
                    .get("withdrawPrecision")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|p| u32::try_from(p).ok()),
                updated_at: Utc::now(),
            });
        }
    }
    Ok(out)
}

/// Флаг сети depositEnable / withdrawEnable — boolean по официальной документации (api-ai-skills, skills/spot-wallet/
/// api-reference.md, раздел config/getall, «depositEnable | bool»). Нет поля или другой тип — ответ не понят:
/// ошибка BadResponse с куском ответа, справочник целиком не обновляется (core/registry_sync.rs оставляет прежний).
/// «Закрыт» по догадке не ставится. Принимает: сеть из networkList, имя поля, тикер. Ошибки: BadResponse.
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

/// Стоимость вывода: комиссия вывода withdrawFee из config/getall (в монетах) — docs-v3.
/// Официально комиссия считается системой по газу, опубликованное значение — справочное
/// (support-статья Fee Schedule); факт — transactionFee в истории выводов.
/// Принимает: монету, сеть, сумму (комиссия от суммы не зависит). Возвращает: [WithdrawFee].
/// Ошибки: монеты или сети нет в справочнике, BingX не сообщил комиссию, связь.
pub async fn withdraw_cost(
    ctx: &Ctx,
    asset: &Asset,
    network: &str,
    _amount: Decimal,
) -> GatewayResult<Vec<Cost>> {
    let all = coin_networks(ctx).await?;
    let info = all
        .iter()
        .find(|c| c.asset.ticker == asset.ticker && c.network == network)
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::NotConfigured,
                None,
                format!("У {} нет сети {network} в справочнике BingX", asset.ticker),
            ))
        })?;
    let fee = info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "BingX не сообщил комиссию вывода {} в сети {network}",
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

/// Адрес депозита монеты в сети. Ошибки: сети нет у монеты, связь.
pub async fn deposit_address(ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<String> {
    let data = rest::signed(
        ctx,
        Method::GET,
        "/openApi/wallets/v1/capital/deposit/address",
        vec![("coin", asset.ticker.clone())],
        Op::Other,
    )
    .await?;
    let list = num::arr(&data, "data")?;
    let found = list
        .iter()
        // status по docs-v3: 0 — адрес активирован, 1 — запрошен, 2 — не запрошен. Нет поля — как в примере docs-v3.
        .find(|a| {
            num::text_opt(a, "network").as_deref() == Some(network)
                && num::text_opt(a, "status").is_none_or(|s| s == "0")
        });
    let found = found.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::InvalidAddress,
            None,
            format!("У {} нет адреса депозита в сети {network}", asset.ticker),
        ))
    })?;
    num::text(found, "address")
}
