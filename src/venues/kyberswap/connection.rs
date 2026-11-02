//! Файл: venues/kyberswap/connection.rs
//! Что делает: «Проверить подключение» площадки kyberswap — API агрегатора (ping эндпоинта /routes), узел BSC по HTTP
//! (сеть 56, номер блока) и кошелёк (ключ → адрес, баланс BNB). Ответ — «Подключено» или точная причина по части.
//! Кто вызывает: venue.rs (check_connection) ← api (кнопка на экране «Площадки»).
//! От чего зависит: ctx.rs, rest.rs, node.rs, balances.rs, signer.rs, state.rs.
//! Настройки: kyber.* , venue.quote_asset [kyberswap]; секреты bsc_dex.wallet_key, kyber.node_http_key.
//!
//! Методы узла — execution-apis (eth_chainId, eth_blockNumber); маршрут /routes — спецификация агрегатора KyberSwap.

use alloy_primitives::U256;

use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::ErrorCode;

use super::balances;
use super::ctx::Ctx;
use super::node::{self, BSC_CHAIN_ID};
use super::quote;
use super::rest;
use super::signer;
use super::state::Parts;

/// Полная проверка. Возвращает: ConnectionCheck (ok — только если все части в порядке). Ошибок нет.
pub async fn check(p: &Parts) -> ConnectionCheck {
    let parts = [
        ("API агрегатора", api(p).await),
        ("Узел BSC (HTTP)", bsc_node(&p.ctx).await),
        ("Кошелёк", wallet(&p.ctx).await),
    ];
    let ok = parts.iter().all(|(_, r)| r.is_ok());
    let details = parts
        .iter()
        .map(|(name, r)| match r {
            Ok(s) => format!("{name}: {s}"),
            Err(e) => format!("{name}: ОШИБКА — {e}"),
        })
        .collect::<Vec<_>>()
        .join("; ");
    ConnectionCheck {
        ok,
        details: if ok {
            format!("Подключено. {details}")
        } else {
            details
        },
    }
}

/// API агрегатора: пробный запрос /routes по реальной паре USDT↔монета из списка подписки. Доступность — это ответ
/// сервера (даже деловая ошибка вроде «нет маршрута» значит, что API отвечает); провал — только сетевые коды
/// (Unreachable/Timeout). До подписки или без второй монеты проверяется только адрес базы и сети.
async fn api(p: &Parts) -> Result<String, String> {
    let base = p.ctx.api_base().map_err(|e| e.message)?;
    let Ok(usdt) = quote::usdt(p) else {
        return Ok(format!(
            "{base} (адрес и сеть заданы; полная проверка маршрута — после подписки, когда известен адрес USDT)"
        ));
    };
    let Some(coin) = p
        .assets()
        .into_iter()
        .find_map(|a| a.bsc_contract.filter(|c| *c != usdt))
    else {
        return Ok(format!(
            "{base} (адрес и сеть заданы; монет для пробного маршрута пока нет)"
        ));
    };
    // Малый объём: 1 wei USDT; агрегатор либо вернёт маршрут, либо деловую ошибку — и то и другое значит «отвечает».
    match rest::routes(&p.ctx, usdt, coin, U256::from(1u8)).await {
        Ok(_) => Ok(format!("{base} отвечает маршрутом")),
        Err(e) if matches!(e.code, ErrorCode::Unreachable | ErrorCode::Timeout) => {
            Err(format!("{base}: {}", e.message))
        }
        Err(e) => Ok(format!("{base} отвечает ({})", e.message)),
    }
}

/// Узел BSC по HTTP: сеть BSC и номер блока.
async fn bsc_node(ctx: &Ctx) -> Result<String, String> {
    let chain: alloy_primitives::U64 = node::call(ctx, "eth_chainId", serde_json::json!([]))
        .await
        .map_err(|e| e.message)?;
    if chain.to::<u64>() != BSC_CHAIN_ID {
        return Err(format!(
            "узел другой сети: chainId {chain}, у BSC mainnet {BSC_CHAIN_ID}"
        ));
    }
    let block = node::block_number(ctx).await.map_err(|e| e.message)?;
    Ok(format!("сеть {chain}, блок {block}"))
}

/// Кошелёк: ключ из секретов → адрес, баланс BNB.
async fn wallet(ctx: &Ctx) -> Result<String, String> {
    let addr = signer::address(ctx).map_err(|e| e.message)?;
    let bnb = balances::bnb(ctx, addr).await.map_err(|e| e.message)?;
    Ok(format!("адрес {addr}, BNB {bnb}"))
}
