//! Файл: venues/kucoin/deposit_addr.rs
//! Что делает: адрес депозита KuCoin по монете и сети (chainId) и счёт, на который он зачисляет (MAIN или TRADE).
//! Кэш в памяти на время процесса (адрес нужен в предпроверке каждой сделки). Адрес с непустым memo — ошибка «требует
//! memo — не поддерживается» (вывод на него без memo не зачислится); такой не кэшируется.
//! Адрес для перевода на KuCoin (for_deposit) — только с зачислением на TRADE (решение 137, Тигран 04.11: адрес BSC с
//! зачислением на TRADE создаётся в кабинете; перевод main → trade после депозита по умолчанию не нужен). Адрес MAIN
//! принимается только при venue.deposit_to_spot = да (запасной вариант: перевод main → trade после зачисления).
//! Решение 208 (07.11.2026, Тигран: «в офф доках об этом прямо написано»): пустой ответ GET — адрес создаётся
//! запросом Add Deposit Address V3 (create) с зачислением на TRADE; раньше — отказ «создайте в кабинете» (живые
//! сделки UMXM и QAIT: перегон на KuCoin не состоялся, у всех монет кроме USDT ответ GET был пустым).
//! Кто вызывает: mod.rs (deposit_address), fund_move.rs (на какой счёт приходит депозит).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.deposit_to_spot [kucoin] (да — допускается адрес MAIN).
//!
//! Официально — Get Deposit Address (V3) GET /api/v3/deposit-addresses?currency=&chain= (пул Management, вес 5)
//! https://www.kucoin.com/docs-new/api-3470140: «If the returned data is empty, you may need to add the deposit address
//! first»; ответ — массив {address, memo («If there's no remark, it is empty … If you do not fill in memo(tag), your
//! deposit may not be available»), chainId, to («MAIN (funding account), TRADE (spot trading account)»), currency,
//! contractAddress, chainName}. Адресов у сети может быть несколько (MAIN и TRADE): берётся TRADE (зачисление сразу на
//! торговый счёт), иначе MAIN — тогда после зачисления нужен перевод main → trade (fund_move.rs).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use reqwest::Method;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::reply::Pool;
use super::{num, rest};

/// Адрес депозита и счёт зачисления («MAIN» или «TRADE», как в ответе).
#[derive(Debug, Clone)]
pub struct DepositAddr {
    pub address: String,
    pub to: String,
}

/// Полученные адреса: (монета, chainId) → адрес. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DepositAddrs {
    map: Arc<RwLock<HashMap<(String, String), DepositAddr>>>,
}

impl DepositAddrs {
    /// Что делает: адрес депозита монеты в сети — из памяти или запросом.
    /// Принимает: окружение, монету, chainId. Возвращает: DepositAddr.
    /// Ошибки: InvalidAddress — адрес не создан (решение 208) или требует memo; связь, коды KuCoin.
    pub async fn get(&self, ctx: &Ctx, asset: &Asset, chain: &str) -> GatewayResult<DepositAddr> {
        let key = (asset.ticker.clone(), chain.to_string());
        if let Some(a) = self.map.read().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return Ok(a.clone());
        }
        let q = vec![
            ("currency", asset.ticker.clone()),
            ("chain", chain.to_string()),
        ];
        let path = "/api/v3/deposit-addresses";
        let data =
            rest::signed(ctx, Method::GET, path, q, None, Op::Other, Pool::Management).await?;
        let list: Vec<&serde_json::Value> = data
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|r| num::text_opt(r, "chainId").as_deref() == Some(chain))
                    .collect()
            })
            .unwrap_or_default();
        let found = list
            .iter()
            .find(|r| num::text_opt(r, "to").is_some_and(|t| t.eq_ignore_ascii_case("TRADE")))
            .or_else(|| list.first())
            .map(|r| (*r).clone());
        // Решение 208: «If the returned data is empty, you may need to add the deposit address first» — адреса нет,
        // создаём его официальным запросом (Add Deposit Address V3), с зачислением на торговый счёт.
        let owned = match found {
            Some(v) => v,
            None => create(ctx, asset, chain).await?,
        };
        let pick = &owned;
        if let Some(memo) = num::text_opt(pick, "memo").filter(|m| !m.trim().is_empty()) {
            return Err(invalid(
                ctx,
                format!(
                    "Адрес депозита KuCoin {} в сети {chain} требует memo ({memo}) — не поддерживается",
                    asset.ticker
                ),
            )
            .with_raw(pick.to_string()));
        }
        let a = DepositAddr {
            address: num::text(pick, "address")?,
            to: num::text_opt(pick, "to")
                .unwrap_or_default()
                .to_ascii_uppercase(),
        };
        self.map
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, a.clone());
        Ok(a)
    }
}

/// Что делает: адрес, на который ядро переводит монету на KuCoin: с зачислением на TRADE; MAIN — только при
/// venue.deposit_to_spot = да (см. шапку). Принимает: адреса, окружение, монету, chainId. Возвращает: адрес.
/// Ошибки: как DepositAddrs::get; InvalidAddress — адрес зачисляет на MAIN, а перевод main → trade выключен.
pub async fn for_deposit(
    addrs: &DepositAddrs,
    ctx: &Ctx,
    asset: &Asset,
    chain: &str,
) -> GatewayResult<String> {
    let a = addrs.get(ctx, asset, chain).await?;
    if a.to != super::fund_move::TRADE && !ctx.flag("venue.deposit_to_spot").unwrap_or(false) {
        let m = format!(
            "Адрес депозита KuCoin {} в сети {chain} зачисляет на {} (не TRADE): монету не продать без перевода — \
             создайте в кабинете KuCoin адрес с зачислением на Trading Account или включите venue.deposit_to_spot",
            asset.ticker, a.to
        );
        return Err(invalid(ctx, m));
    }
    Ok(a.address)
}

/// Что делает: решение 208 — создать адрес депозита монеты в сети с зачислением на торговый счёт.
/// Официально — Add Deposit Address (V3) POST /api/v3/deposit-address/create (право General, пул Management, вес 20)
/// https://www.kucoin.com/docs-new/rest/account-info/deposit/add-deposit-address-v3: пример тела {"currency": "TON",
/// "chain": "ton", "to": "trade"}, ответ data {address, memo, chainId, to, currency, chainName}.
/// Принимает: окружение, монету, chainId. Возвращает: объект адреса из ответа.
/// Ошибки: InvalidAddress — в ответе нет адреса или сеть другая; связь, коды KuCoin (как у rest::signed).
async fn create(ctx: &Ctx, asset: &Asset, chain: &str) -> GatewayResult<serde_json::Value> {
    let body = serde_json::json!({ "currency": asset.ticker, "chain": chain, "to": "trade" });
    let path = "/api/v3/deposit-address/create";
    let data = rest::signed(
        ctx,
        Method::POST,
        path,
        vec![],
        Some(body),
        Op::Other,
        Pool::Management,
    )
    .await?;
    let same_chain = num::text_opt(&data, "chainId").as_deref() == Some(chain);
    if num::text_opt(&data, "address").is_none() || !same_chain {
        return Err(invalid(
            ctx,
            format!(
                "KuCoin не создал адрес депозита {} в сети {chain}",
                asset.ticker
            ),
        )
        .with_raw(data.to_string()));
    }
    let m = format!(
        "Создан адрес депозита KuCoin {} в сети {chain} (зачисление на {})",
        asset.ticker,
        num::text_opt(&data, "to").unwrap_or_default()
    );
    ctx.log(
        crate::storage::journal::Level::Info,
        "venue.deposit_address",
        m,
    )
    .await;
    Ok(data)
}

/// Ошибка InvalidAddress с площадкой KuCoin.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
