//! Файл: venues/mexc/deposit_addr_new.rs
//! Что делает: решение 379 (задача t165, Тигран 17.11: «на всех биржах этот адрес должен автоматом создаваться») —
//! создание адреса депозита MEXC, когда в ответе Deposit Address его нет, и общий выбор записи сети из ответа (один и
//! тот же для GET и POST /api/v3/capital/deposit/address). Раньше — отказ «создайте его в кабинете MEXC».
//! Кто вызывает: deposit_addr.rs (deposit_address), тесты (pick — tests/mexc_deposit_addr_t165.rs).
//! От чего зависит: rest.rs, num.rs, ctx.rs (журнал), storage/journal.rs.
//! Настройки: нет своих (запрет создания у прогрева — gateway/addr_claim.rs, venue.deposit_warm_create читает ядро).
//!
//! Официально — Wallet Endpoints → «Generate deposit address (supporting network)» POST /api/v3/capital/deposit/address
//! (https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/generate-deposit-address-supporting-network, прочитано
//! 17.11.2026): право «SPOT_WITHDRAW_WRITE», вес (IP) 1; параметры coin (YES), network (YES, «deposit network»),
//! timestamp, signature; ответ — address, coin, memo, network; пример ответа — массив записей (в прежней странице
//! mexcdevelop.github.io — тот же массив). Какое имя сети ждёт network (код или отображаемое, как «BEP20(BSC)»),
//! документация не говорит — передаётся то же имя, что и в GET (deposit_addr.rs: отображаемое из справочника сетей),
//! запись ответа выбирается тем же pick (имя или код). Что будет при повторном создании и предел числа адресов —
//! не описаны: создаётся только когда GET адреса не дал, после создания адрес кэшируется и больше не создаётся.
//! Записанного живого ответа нет: тест выбора — на примере из документации (tests/data/mexc_doc_2026-11-17); живой
//! ответ — в сыром журнале сервера (<data-dir>/raw/mexc/*.jsonl.zst) при первом создании (docs/TASKS.md).

use reqwest::Method;
use serde_json::Value;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::errors::Op;
use super::{num, rest};

/// Что делает: запись адреса сети из ответа Deposit Address / Generate deposit address — по отображаемому имени сети
/// или по её коду. Ответ — массив записей (пример документации обоих методов); объект — одна запись (берётся, если
/// сеть совпадает). Принимает: ответ, отображаемое имя сети, код сети. Возвращает: запись или None. Ошибок нет.
pub fn pick<'a>(data: &'a Value, display: &str, network: &str) -> Option<&'a Value> {
    let same = |r: &Value| {
        let n = num::text_opt(r, "network");
        n.as_deref() == Some(display) || n.as_deref() == Some(network)
    };
    match data {
        Value::Array(a) => a.iter().find(|r| same(r)),
        Value::Object(_) if same(data) => Some(data),
        _ => None,
    }
}

/// Что делает: создать адрес депозита монеты в сети и вернуть запись адреса сети из ответа.
/// Принимает: окружение, монету, отображаемое имя сети (как в GET), код сети.
/// Возвращает: запись (address, memo, network) — memo проверяет вызывающий, как у записи GET. После создания — журнал
/// venue.deposit_address (Info, без самого адреса — как KuCoin, решение 208).
/// Ошибки: код и сообщение MEXC дословно (rest::signed) с пояснением, что не удалось создать; InvalidAddress — в
/// ответе нет записи сети или адреса (с сырым ответом).
pub async fn create(
    ctx: &Ctx,
    asset: &Asset,
    display: &str,
    network: &str,
) -> GatewayResult<Value> {
    let path = "/api/v3/capital/deposit/address";
    let p = vec![
        ("coin", asset.ticker.clone()),
        ("network", display.to_string()),
    ];
    let data = rest::signed(ctx, Method::POST, path, p, Op::Other)
        .await
        .map_err(|mut e| {
            e.message = format!(
                "MEXC отказал в создании адреса депозита {} в сети {display} (POST {path}): {}",
                asset.ticker, e.message
            );
            e
        })?;
    let found = pick(&data, display, network)
        .filter(|r| num::text_opt(r, "address").is_some_and(|a| !a.trim().is_empty()));
    let Some(found) = found.cloned() else {
        let m = format!(
            "MEXC не вернул созданный адрес депозита {} в сети {display}: в ответе POST {path} нет записи сети с address",
            asset.ticker
        );
        let e = GatewayError::new(ErrorCode::InvalidAddress, None, m).with_raw(data.to_string());
        return Err(ctx.tag(e));
    };
    let m = format!(
        "Создан адрес депозита MEXC {} в сети {display}",
        asset.ticker
    );
    let _ = ctx
        .journal
        .write(Level::Info, "venue.deposit_address", Some(ID), m, None)
        .await;
    Ok(found)
}
