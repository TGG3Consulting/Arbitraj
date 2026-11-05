//! Файл: venues/htx/deposit_addr.rs
//! Что делает: адрес депозита HTX по монете и сети ядра. Кэш в памяти на время процесса (адрес нужен в предпроверке
//! каждой сделки до покупки — урок 25, решение 118). Адрес с непустым addressTag — ошибка «требует tag — не
//! поддерживается» (вывод на него без tag не зачислится); такой не кэшируется.
//! Кто вызывает: mod.rs (deposit_address).
//! От чего зависит: rest.rs, wallet.rs (chain HTX по сети ядра), num.rs, ctx.rs.
//! Настройки: через wallet.rs — venue.bsc_network.
//!
//! Официально — Query Deposit Address GET /v2/account/deposit/address?currency= («20times/2s», право Read)
//! https://huobiapi.github.io/docs/spot/v1/en/#query-deposit-address: data[] {currency, address, addressTag, chain} —
//! по адресу на каждую сеть монеты. На какой счёт зачисляется депозит, ответ не говорит; отдельного счёта фонда у спот-API
//! HTX нет — зачисление на спот (подтвердить первым депозитом, README.md папки).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde_json::Value;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::{Ctx, currency};
use super::{num, rest, wallet};

/// Полученные адреса: (тикер, сеть ядра) → адрес. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DepositAddrs {
    map: Arc<RwLock<HashMap<(String, String), String>>>,
}

impl DepositAddrs {
    /// Что делает: адрес депозита монеты в сети — из памяти или запросом.
    /// Принимает: окружение, монету, сеть ядра. Возвращает: адрес.
    /// Ошибки: InvalidAddress — адреса нет или адрес требует tag; NotConfigured — сети нет; связь, коды HTX.
    pub async fn get(&self, ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<String> {
        let key = (asset.ticker.clone(), network.to_string());
        if let Some(a) = self.map.read().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return Ok(a.clone());
        }
        let chain = wallet::chain_of(ctx, &asset.ticker, network).await?;
        let q = vec![("currency", currency(&asset.ticker))];
        let data = rest::get(ctx, "/v2/account/deposit/address", q).await?;
        let address = pick(&data, &chain).map_err(|m| {
            let m = format!("HTX {} ({network}, chain {chain}): {m}", asset.ticker);
            ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, m))
                .with_raw(data.to_string())
        })?;
        self.map
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, address.clone());
        Ok(address)
    }
}

/// Адрес сети chain из data. Ошибка — текст причины (нет адреса этой сети; адрес требует tag).
pub fn pick(data: &Value, chain: &str) -> Result<String, String> {
    let rec = data
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|r| num::text_opt(r, "chain").as_deref() == Some(chain))
        })
        .ok_or("нет адреса депозита этой сети — проверьте в кабинете HTX (Deposit)")?;
    if let Some(tag) = num::text_opt(rec, "addressTag").filter(|t| !t.trim().is_empty()) {
        return Err(format!(
            "адрес депозита требует tag ({tag}) — не поддерживается"
        ));
    }
    num::text_opt(rec, "address").ok_or_else(|| "в ответе нет address".into())
}
