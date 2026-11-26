//! Файл: venues/bsc_dex/node/chain_id.rs
//! Что делает: решение 416 (шаг 1 двух сетей) — сеть площадки DEX из настроек вместо зашитого chainId 56: номер цепи EVM
//! (dex.evm_chain_id) для подписи транзакций (EIP-155) и сверка с eth_chainId узла — в «Проверить подключение»
//! (connection.rs, connection_extra.rs) и при подписке на рынок (venue.rs subscribe_market: узел другой сети — подписки
//! нет). Id сети в боте (dex.chain) при подписке тоже обязателен: не задан — подписки нет.
//! Кто вызывает: send/tx.rs (chain_id — до отправки, решение 398), exec/txs.rs, exec/bundle_sim.rs, connection*.rs,
//! venue.rs.
//! От чего зависит: ctx.rs, node/rpc.rs.
//! Настройки: dex.evm_chain_id, dex.chain [bsc_dex].
//!
//! eth_chainId — https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth ;
//! chainId в подписи — https://eips.ethereum.org/EIPS/eip-155 .

use std::sync::Mutex;

use alloy_primitives::U64;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::rpc;

/// Что делает: номер цепи EVM площадки. Возвращает: chainId. Ошибки: NotConfigured — dex.evm_chain_id не задана.
pub fn chain_id(ctx: &Ctx) -> GatewayResult<u64> {
    let id = ctx.int("dex.evm_chain_id")?;
    if let Some(p) = crate::settings::evm_profiles::profile(ctx.place.id)
        && (p.evm_chain_id != id || ctx.text("dex.chain")? != p.chain)
    {
        return Err(ctx.not_set("dex.evm_chain_id", format!("{}: ожидается {} (chainId {})", p.venue, p.chain, p.evm_chain_id)));
    }
    Ok(id)
}

/// Что делает: сверка ответа eth_chainId узла с dex.evm_chain_id. Принимает: окружение, chainId узла.
/// Возвращает: Ok или текст причины (настройка не задана; узел другой сети).
pub fn same_chain(ctx: &Ctx, got: u64) -> Result<(), String> {
    let want = chain_id(ctx).map_err(|e| e.message)?;
    if got == want {
        Ok(())
    } else {
        Err(format!(
            "узел другой сети: chainId {got}, у площадки {want} (dex.evm_chain_id)"
        ))
    }
}

/// Что делает: часть «Сеть площадки» в «Проверить подключение» — заданы ли dex.chain и dex.evm_chain_id.
/// Принимает: окружение. Возвращает: описание («bsc, chainId 56») или текст причины.
pub fn describe(ctx: &Ctx) -> Result<String, String> {
    let id = ctx.text("dex.chain").map_err(|e| e.message)?;
    if id.trim().is_empty() {
        return Err("dex.chain пуста — задайте id сети площадки DEX".into());
    }
    let n = chain_id(ctx).map_err(|e| e.message)?;
    Ok(format!("{id}, chainId {n}"))
}

/// Что делает: проверка сети перед подпиской на рынок — dex.chain и dex.evm_chain_id заданы, eth_chainId узла равен
/// dex.evm_chain_id. Узел не ответил — подписка идёт как раньше (её собственные запросы покажут сбой связи), в журнал
/// venue.chain_check — предупреждение; узел другой сети — подписки нет.
/// Принимает: окружение. Ошибки: NotConfigured — настройка не задана; BadResponse — узел другой сети.
pub async fn check_start(ctx: &Ctx) -> GatewayResult<()> {
    if ctx.text("dex.chain")?.trim().is_empty() {
        return Err(ctx.not_set(
            "dex.chain",
            "dex.chain пуста — задайте id сети площадки DEX",
        ));
    }
    let want = chain_id(ctx)?;
    match rpc::call::<U64>(ctx, "eth_chainId", json!([])).await {
        Ok(got) if got.to::<u64>() == want => {
            warn_once(ctx.place.id, None);
            Ok(())
        }
        Ok(got) => Err(ctx.err(
            ErrorCode::BadResponse,
            format!("узел другой сети: chainId {got}, у площадки {want} (dex.evm_chain_id) — подписки на рынок нет"),
        )),
        Err(e) if crate::settings::evm_profiles::profile(ctx.place.id).is_some() => Err(e),
        Err(e) => {
            let m = format!(
                "сеть узла не сверена перед подпиской (eth_chainId): {} — подписка идёт, сверка — в «Проверить подключение»",
                e.message
            );
            // Ревью М-2: одна запись на причину (повтор подписки с той же причиной журнал не засоряет).
            if warn_once(ctx.place.id, Some(&e.message)) {
                ctx.log(Level::Warn, "venue.chain_check", m, None).await;
            }
            Ok(())
        }
    }
}

/// Последняя записанная причина «сеть узла не сверена» по площадке DEX (ревью М-2; решение 435, 433-и: предупреждение
/// одной сети не глушит другую).
static LAST_WARN: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());

/// Что делает: дедуп предупреждения площадки — Some(причина): true, если причина новая (запомнена); None — сверка
/// прошла, память площадки очищена. Принимает: площадку DEX, причину. Ошибок нет.
fn warn_once(venue: &'static str, why: Option<&str>) -> bool {
    let mut last = LAST_WARN.lock().unwrap_or_else(|p| p.into_inner());
    let had = last.iter().position(|(v, _)| *v == venue);
    match (why, had) {
        (None, Some(i)) => {
            last.remove(i);
            false
        }
        (None, None) => false,
        (Some(w), Some(i)) if last[i].1 == w => false,
        (Some(w), Some(i)) => {
            last[i].1 = w.to_string();
            true
        }
        (Some(w), None) => {
            last.push((venue, w.to_string()));
            true
        }
    }
}
