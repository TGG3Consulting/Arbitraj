//! Файл: venues/bingx/wallet.rs
//! Что делает: справочник монет и сетей BingX и адрес депозита.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, core/types.
//! Настройки: venue.bsc_network.
//!
//! Официальная документация (docs-v3 и api-ai-skills/spot-wallet):
//! - GET /openApi/wallets/v1/capital/config/getall — networkList: network, depositEnable, withdrawEnable,
//!   withdrawFee, withdrawMin, withdrawMax, minConfirm, contractAddress (поле есть в api-ai-skills от
//!   2026-10-22; в примере docs-v3 его нет — проверяется на реальном ответе).
//! - GET /openApi/wallets/v1/capital/deposit/address — data.data[]: coin, network, address, tag.

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo};
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
                venue: ctx.venue(),
                asset: Asset {
                    ticker: ticker.clone(),
                    bsc_contract: contract,
                },
                network,
                deposit_enabled: n
                    .get("depositEnable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                withdraw_enabled: n
                    .get("withdrawEnable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                withdraw_fee: num::dec_opt(n, "withdrawFee")?,
                withdraw_min: num::dec_opt(n, "withdrawMin")?,
                withdraw_max: num::dec_opt(n, "withdrawMax")?,
                deposit_confirmations: num::text_opt(n, "minConfirm").and_then(|s| s.parse().ok()),
                updated_at: Utc::now(),
            });
        }
    }
    Ok(out)
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
        .find(|a| num::text_opt(a, "network").as_deref() == Some(network));
    let found = found.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::InvalidAddress,
            None,
            format!("У {} нет адреса депозита в сети {network}", asset.ticker),
        ))
    })?;
    num::text(found, "address")
}
