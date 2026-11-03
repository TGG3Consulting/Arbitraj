//! Файл: venues/bsc_dex/exec/executor/onchain.rs
//! Что делает: проверка развёрнутого исполнителя по адресу exec.executor_address (ревью 122, пункт 1) — адрес вводится
//! руками, поэтому до любого approve и до сделки сверяется: код по адресу (eth_getCode) побайтно равен
//! contracts/out/ArbExecutor.bin-runtime (как после развёртывания, deploy.rs) и слот владельца (раскладка solc,
//! code.rs::owner_slot) хранит кошелёк бота. Не совпало — ни разрешения, ни сделки, ошибка с точной причиной.
//! Успешная проверка помнится по паре (адрес, кошелёк) — до смены настройки или кошелька (другая пара — проверка
//! заново); отказ не запоминается. Кнопка «Проверить подключение» исполнителя (экран площадки DEX) — probe.
//! Кто вызывает: exec/executor/{trade, dry}.rs (ensure), venue.rs (check_executor → probe).
//! От чего зависит: exec/executor/{code, mod}.rs, node/rpc.rs, wallet/signer.rs.
//! Настройки: exec.executor_address [bsc_dex].
//!
//! eth_getCode / eth_getStorageAt — execution-apis 16caee27 src/eth/state.yaml.

use std::sync::Mutex;

use alloy_primitives::{Address, B256, Bytes};
use serde_json::json;

use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::ctx::Ctx;
use super::super::super::node::rpc;
use super::super::super::wallet::signer;
use super::code;

/// Последняя успешно проверенная пара (адрес исполнителя, кошелёк).
static CHECKED: Mutex<Option<(Address, Address)>> = Mutex::new(None);

/// Исполнитель по адресу — наш и принадлежит кошельку. Принимает: адрес исполнителя, кошелёк бота.
/// Ошибки: InvalidAddress (нет кода / код не тот / владелец не кошелёк — текст причины), Internal (артефакт), узел.
pub async fn ensure(ctx: &Ctx, executor: Address, wallet: Address) -> GatewayResult<()> {
    let key = (executor, wallet);
    if *CHECKED.lock().unwrap_or_else(|p| p.into_inner()) == Some(key) {
        return Ok(());
    }
    match inspect(ctx, executor, wallet).await? {
        Ok(_) => {
            *CHECKED.lock().unwrap_or_else(|p| p.into_inner()) = Some(key);
            Ok(())
        }
        Err(why) => Err(ctx.err(
            ErrorCode::InvalidAddress,
            format!("exec.executor_address: {why} — разрешения и сделки через исполнитель нет"),
        )),
    }
}

/// Сверка с узла (без памяти). Возвращает: Ok(описание) или Err(точная причина). Ошибки: Internal (артефакт), узел.
async fn inspect(
    ctx: &Ctx,
    executor: Address,
    wallet: Address,
) -> GatewayResult<Result<String, String>> {
    let want = code::runtime().map_err(|w| ctx.err(ErrorCode::Internal, w))?;
    let have: Bytes = rpc::call(ctx, "eth_getCode", json!([executor, "latest"])).await?;
    if have.is_empty() {
        return Ok(Err(format!("по адресу {executor:#x} нет кода контракта")));
    }
    if have != want {
        return Ok(Err(format!(
            "код по адресу {executor:#x} ({} байт) не совпал с contracts/out/ArbExecutor.bin-runtime ({} байт)",
            have.len(),
            want.len()
        )));
    }
    let slot = code::owner_slot().map_err(|w| ctx.err(ErrorCode::Internal, w))?;
    let word: B256 = rpc::call(ctx, "eth_getStorageAt", json!([executor, slot, "latest"])).await?;
    let owner = Address::from_word(word);
    if owner != wallet {
        return Ok(Err(format!(
            "владелец исполнителя {owner:#x} — не кошелёк бота {wallet:#x}"
        )));
    }
    Ok(Ok(format!(
        "исполнитель {executor:#x}: код совпал с репозиторием, владелец — кошелёк бота {wallet:#x}"
    )))
}

/// Кнопка «Проверить подключение» исполнителя: «Подключено …» или точная причина (адрес не задан, ключ кошелька,
/// нет кода, код не тот, владелец не кошелёк, узел). Ошибок нет: причина — в details.
pub async fn probe(ctx: &Ctx) -> ConnectionCheck {
    let res = async {
        let Some(executor) = super::address(ctx).map_err(|e| e.message)? else {
            return Err("exec.executor_address не задан".to_string());
        };
        let wallet = signer::address(ctx).map_err(|e| e.message)?;
        inspect(ctx, executor, wallet)
            .await
            .map_err(|e| e.message)?
    }
    .await;
    match res {
        Ok(s) => ConnectionCheck {
            ok: true,
            details: format!("Подключено: {s}"),
        },
        Err(why) => ConnectionCheck {
            ok: false,
            details: why,
        },
    }
}
