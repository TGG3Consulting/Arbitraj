//! Файл: venues/phemex/deposit_addr.rs
//! Что делает: адрес депозита Phemex по монете и сети (chainName). Кэш в памяти на время процесса (адрес нужен в
//! предпроверке каждой сделки до покупки — урок 25, решение 118). Адрес с непустым tag — ошибка «требует tag — не
//! поддерживается» (вывод на него без tag не зачислится); такой не кэшируется.
//! Кто вызывает: mod.rs (deposit_address).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официально — Query deposit address information GET /phemex-deposit/wallets/api/depositAddress?currency=&chainName=
//! (https://phemex-docs.github.io/#query-deposit-address-information): data {address, tag «chain address tag»}. На какой
//! счёт (SPOT / Contract) зачисляется депозит, параметра у адреса нет — см. fund_move.rs и deposits.rs.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use reqwest::Method;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Полученные адреса: (монета, chainName) → адрес. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DepositAddrs {
    map: Arc<RwLock<HashMap<(String, String), String>>>,
}

impl DepositAddrs {
    /// Что делает: адрес депозита монеты в сети — из памяти или запросом.
    /// Принимает: окружение, монету, chainName. Возвращает: адрес.
    /// Ошибки: InvalidAddress — адреса нет или адрес требует tag; связь, коды Phemex.
    pub async fn get(&self, ctx: &Ctx, asset: &Asset, chain: &str) -> GatewayResult<String> {
        let key = (asset.ticker.clone(), chain.to_string());
        if let Some(a) = self.map.read().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return Ok(a.clone());
        }
        let q = vec![
            ("currency", asset.ticker.clone()),
            ("chainName", chain.to_string()),
        ];
        let path = "/phemex-deposit/wallets/api/depositAddress";
        let data = rest::signed(ctx, Method::GET, path, q, None, Op::Other).await?;
        let raw = data.to_string();
        if let Some(tag) = num::text_opt(&data, "tag").filter(|t| !t.trim().is_empty()) {
            let m = format!(
                "Адрес депозита Phemex {} в сети {chain} требует tag ({tag}) — не поддерживается",
                asset.ticker
            );
            return Err(invalid(ctx, m).with_raw(raw));
        }
        let address = num::text_opt(&data, "address").ok_or_else(|| {
            let m = format!(
                "У {} нет адреса депозита Phemex в сети {chain} — проверьте в кабинете Phemex (Deposit)",
                asset.ticker
            );
            invalid(ctx, m).with_raw(raw.clone())
        })?;
        self.map
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, address.clone());
        Ok(address)
    }
}

/// Ошибка InvalidAddress с площадкой Phemex.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
