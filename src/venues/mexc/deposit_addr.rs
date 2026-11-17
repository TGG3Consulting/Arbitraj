//! Файл: venues/mexc/deposit_addr.rs
//! Что делает: адрес депозита MEXC по монете и сети. Кэш в памяти по (монета, сеть) на время работы процесса
//! (решение 118, Я-5: адрес нужен в предпроверке каждой сделки — повторно запросов нет). Адрес с непустым memo —
//! ошибка «требует memo — не поддерживается» (Я-15: вывод на такой адрес без memo не зачислится); такой не кэшируется.
//! Кто вызывает: mod.rs (deposit_address из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, wallet.rs (имя сети, справочник), symbols.rs, deposit_addr_new.rs
//! (создание, выбор записи сети), gateway/addr_claim.rs.
//! Настройки: нет своих (через wallet.rs — venue.bsc_network, venue.networks_fresh_ms).
//!
//! Официальная документация: GET /api/v3/capital/deposit/address?coin=&network= (SPOT_WITHDRAW_READ, вес 10)
//! https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/deposit-address-supporting-network — ответ — массив: coin,
//! network (в примере «BEP20(BSC)» — отображаемое имя), address, memo.
//! Решение 379 (задача t165, Тигран 17.11): адреса сети в ответе нет — бот создаёт его сам, «Generate deposit address
//! (supporting network)» POST того же пути (deposit_addr_new.rs); прогрев адресов без venue.deposit_warm_create = да не
//! создаёт — NotApplicable (gateway/addr_claim.rs, как KuCoin). Раньше — отказ «создайте его в кабинете MEXC».

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use reqwest::Method;

use crate::core::types::Asset;
use crate::gateway::addr_claim::{self, Missing};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::deposit_addr_new;
use super::errors::Op;
use super::symbols::Symbols;
use super::wallet::{self, Networks};
use super::{num, rest};

/// Полученные адреса депозита: (монета, код сети) → адрес. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DepositAddrs {
    map: Arc<RwLock<HashMap<(String, String), String>>>,
}

/// Что делает: адрес депозита монеты в сети (код netWork) — из памяти или запросом по отображаемому имени сети
/// (ответ — запись с этим именем или кодом).
/// Принимает: окружение, кэш адресов, справочник сетей, справочник пар, монету, код сети.
/// Возвращает: адрес.
/// Официально: Deposit Address GET и Generate deposit address POST /api/v3/capital/deposit/address
/// (https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/deposit-address-supporting-network и
/// …/generate-deposit-address-supporting-network).
/// Ошибки: InvalidAddress — адрес требует memo или ответ создания без адреса; NotApplicable — адреса нет, а создание
/// запрещено признаком задачи (прогрев, gateway/addr_claim.rs); отказ MEXC в создании — его код и сообщение
/// (deposit_addr_new.rs); связь, коды MEXC.
pub async fn deposit_address(
    ctx: &Ctx,
    addrs: &DepositAddrs,
    nets: &Networks,
    symbols: &Symbols,
    asset: &Asset,
    network: &str,
) -> GatewayResult<String> {
    let key = (asset.ticker.clone(), network.to_string());
    if let Some(a) = addrs
        .map
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(&key)
    {
        return Ok(a.clone());
    }
    if nets.display(&asset.ticker, network).is_none() {
        wallet::coin_networks(ctx, nets, symbols).await?;
    }
    let display = nets
        .display(&asset.ticker, network)
        .unwrap_or_else(|| network.to_string());
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v3/capital/deposit/address",
        vec![("coin", asset.ticker.clone()), ("network", display.clone())],
        Op::Other,
    )
    .await?;
    // Решение 379: записи сети нет — создать (предпроверка, перевод) или NotApplicable (прогрев без «да»).
    let created;
    let found = match deposit_addr_new::pick(&data, &display, network) {
        Some(r) => r,
        None => match addr_claim::on_missing() {
            Missing::Forbidden => {
                let m = addr_claim::forbidden_text("MEXC", &asset.ticker, &display);
                return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, m)));
            }
            Missing::Create => {
                created = deposit_addr_new::create(ctx, asset, &display, network).await?;
                &created
            }
        },
    };
    if let Some(memo) = num::text_opt(found, "memo").filter(|m| !m.trim().is_empty()) {
        return Err(invalid(
            ctx,
            format!(
                "Адрес депозита MEXC {} в сети {display} требует memo ({memo}) — не поддерживается",
                asset.ticker
            ),
        )
        .with_raw(found.to_string()));
    }
    let address = num::text(found, "address")?;
    addrs
        .map
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, address.clone());
    Ok(address)
}

/// Ошибка InvalidAddress с площадкой MEXC.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
