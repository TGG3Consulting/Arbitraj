//! Файл: venues/coinex/deposit_addr.rs
//! Что делает: адрес депозита CoinEx по монете и сети (chain). Кэш в памяти на время процесса (адрес нужен в
//! предпроверке каждой сделки до покупки — урок 25, решение 118). Адрес с непустым memo — ошибка «требует memo — не
//! поддерживается» (вывод на него без memo не зачислится); такой не кэшируется.
//! Кто вызывает: mod.rs (deposit_address).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официально — Get Deposit Address GET /assets/deposit-address?ccy=&chain= (оба обязательны; подпись, группа «Query
//! spot accounts (10r/1s)») https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/get-deposit-address — data
//! {address, memo («A memo is required for certain currency deposits»)}. «Deposit addresses can be changed» (Update
//! Deposit Address): бот адрес не меняет; сменили в кабинете — кэш устарел до перезапуска (урок 25 — вопрос Тиграну, нужен
//! ли срок кэша). Зачисление — на спот-счёт: другого счёта для депозита у CoinEx нет (Transfer: счета SPOT, MARGIN,
//! FUTURES; funding-счёта нет) — проверить первым депозитом.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use reqwest::Method;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Полученные адреса: (монета, chain) → адрес. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DepositAddrs {
    map: Arc<RwLock<HashMap<(String, String), String>>>,
}

impl DepositAddrs {
    /// Что делает: адрес депозита монеты в сети — из памяти или запросом.
    /// Принимает: окружение, монету, chain. Возвращает: адрес.
    /// Ошибки: InvalidAddress — адреса нет или адрес требует memo; связь, коды CoinEx.
    pub async fn get(&self, ctx: &Ctx, asset: &Asset, chain: &str) -> GatewayResult<String> {
        let key = (asset.ticker.clone(), chain.to_string());
        if let Some(a) = self.map.read().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return Ok(a.clone());
        }
        let q = vec![("ccy", asset.ticker.clone()), ("chain", chain.to_string())];
        let path = "/v2/assets/deposit-address";
        let data = rest::signed(ctx, Method::GET, path, q, None, Op::Other).await?;
        let raw = data.to_string();
        if let Some(memo) = num::text_opt(&data, "memo").filter(|t| !t.trim().is_empty()) {
            let m = format!(
                "Адрес депозита CoinEx {} в сети {chain} требует memo ({memo}) — не поддерживается",
                asset.ticker
            );
            return Err(invalid(ctx, m).with_raw(raw));
        }
        let address = num::text_opt(&data, "address").ok_or_else(|| {
            let m = format!(
                "У {} нет адреса депозита CoinEx в сети {chain} — проверьте в кабинете CoinEx (Deposit)",
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

/// Ошибка InvalidAddress с площадкой CoinEx.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
