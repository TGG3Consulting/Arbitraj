//! Файл: venues/poloniex/deposit_addr.rs
//! Что делает: адрес депозита Poloniex по монете и сети (blockchain). Кэш в памяти на время процесса (адрес нужен в
//! предпроверке каждой сделки до покупки — урок 25, решение 118). Сеть с memo (address-payment-id) — ошибка «требует
//! memo — не поддерживается»; такой адрес не кэшируется.
//! Кто вызывает: mod.rs (deposit_address).
//! От чего зависит: rest.rs, wallet.rs (one_raw), networks_parse.rs (chain_coin), deposit_addr_new.rs (создание,
//! проверка адреса), gateway/addr_claim.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официально (https://api-docs.poloniex.com/spot/api/private/wallet): Deposit Addresses GET /wallets/addresses?currency=
//! — ответ объект «имя монеты → адрес» («50» на VIP0); параметра сети у запроса НЕТ. Для монеты с несколькими сетями
//! имя монеты в нужной сети — поле coin сети в /v2/currencies/{currency} (networkList; в справочнике v1 такие имена —
//! «USDTETH», «USDTTRON») — по нему и запрашивается адрес (проверить на живом ответе для сети BSC).
//! Решение 379 (задача t165, Тигран 17.11): адреса нет — бот создаёт его сам, New Currency Address POST /wallets/address
//! с тем же именем монеты сети (deposit_addr_new.rs); прогрев адресов без venue.deposit_warm_create = да не создаёт —
//! NotApplicable (gateway/addr_claim.rs, как KuCoin). Раньше — отказ «создайте адрес в кабинете Poloniex» (живой факт
//! 17.11: ESIM bsc_dex → poloniex — 94 отказа предпроверки подряд, BANK → poloniex — 6, сделки потеряны).
//! Счёт зачисления — спот: «accountType … SPOT is the default and only supported one» (Account Information).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use reqwest::Method;

use crate::core::types::Asset;
use crate::gateway::addr_claim::{self, Missing};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::deposit_addr_new;
use super::errors::Op;
use super::networks_parse::{PAYMENT_ID, chain_coin};
use super::{rest, wallet};

/// Полученные адреса: (монета, blockchain) → адрес. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DepositAddrs {
    map: Arc<RwLock<HashMap<(String, String), String>>>,
}

impl DepositAddrs {
    /// Что делает: адрес депозита монеты в сети — из памяти или запросом.
    /// Принимает: окружение, монету, blockchain. Возвращает: адрес.
    /// Официально: Deposit Addresses GET /wallets/addresses, New Currency Address POST /wallets/address
    /// (https://api-docs.poloniex.com/spot/api/private/wallet).
    /// Ошибки: InvalidAddress — сети нет, сеть с memo, в ответе создания нет адреса; NotApplicable — адреса нет, а
    /// создание запрещено признаком задачи (прогрев, gateway/addr_claim.rs); отказ Poloniex в создании — его код и
    /// сообщение (deposit_addr_new.rs); связь, коды Poloniex.
    pub async fn get(&self, ctx: &Ctx, asset: &Asset, chain: &str) -> GatewayResult<String> {
        let key = (asset.ticker.clone(), chain.to_string());
        if let Some(a) = self.map.read().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return Ok(a.clone());
        }
        let data = wallet::one_raw(ctx, &asset.ticker).await?;
        let (coin, ctype) = chain_coin(&data, &asset.ticker, chain).ok_or_else(|| {
            let m = format!(
                "У {} нет сети {chain} в справочнике Poloniex /v2/currencies",
                asset.ticker
            );
            invalid(ctx, m)
        })?;
        if ctype == PAYMENT_ID {
            let m = format!(
                "Депозит Poloniex {} в сети {chain} требует memo (currencyType = {PAYMENT_ID}) — не поддерживается",
                asset.ticker
            );
            return Err(invalid(ctx, m));
        }
        if coin.is_empty() {
            let m = format!(
                "Poloniex: у сети {chain} монеты {} нет поля coin",
                asset.ticker
            );
            return Err(invalid(ctx, m).with_raw(data.to_string()));
        }
        let q = vec![("currency", coin.clone())];
        let got = rest::signed(ctx, Method::GET, "/wallets/addresses", q, None, Op::Other).await?;
        // Решение 379: адреса в ответе нет — создать (предпроверка, перевод) или NotApplicable (прогрев без «да»).
        let address = match deposit_addr_new::pick(&got, &coin) {
            Some(a) => a,
            None => match addr_claim::on_missing() {
                Missing::Forbidden => {
                    let m = addr_claim::forbidden_text("Poloniex", &asset.ticker, chain);
                    return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, m)));
                }
                Missing::Create => {
                    deposit_addr_new::create(ctx, &asset.ticker, chain, &coin).await?
                }
            },
        };
        self.map
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, address.clone());
        Ok(address)
    }
}

/// Ошибка InvalidAddress с площадкой Poloniex.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
