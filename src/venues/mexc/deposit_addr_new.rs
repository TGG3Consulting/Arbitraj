//! Файл: venues/mexc/deposit_addr_new.rs
//! Что делает: решение 379 (задача t165, Тигран 17.11: «на всех биржах этот адрес должен автоматом создаваться») —
//! запрос адреса депозита (GET), создание адреса MEXC (POST), когда записи сети в ответе GET нет, и общий выбор записи
//! сети из ответа (`pick`, один и тот же для GET и POST /api/v3/capital/deposit/address). Раньше — отказ «создайте его
//! в кабинете MEXC».
//! Правки по ревью t165:
//! - Б1: адрес после создания берётся ТОЛЬКО из записи сети повторного GET; адрес записи POST лишь сверяется с ним
//!   (gateway/addr_create.rs::confirm). Нет в GET или не совпал — InvalidAddress (текст без адреса), в кэш не идёт;
//! - Н1: POST — класс операции Op::Create: на резервный адрес REST только если запрос точно не отправлен (как ордер и
//!   вывод); после таймаута — тот же повторный GET, повторного POST нет;
//! - Н3: в тексте отказа POST — переданное значение network (какое имя сети ждёт MEXC, документация не говорит).
//!
//! Кто вызывает: deposit_addr.rs (deposit_address), тесты (pick — tests/mexc_deposit_addr_t165.rs).
//! От чего зависит: rest.rs, num.rs, ctx.rs (журнал), gateway/addr_create.rs, storage/journal.rs.
//! Настройки: нет своих (запрет создания у прогрева — gateway/addr_claim.rs, venue.deposit_warm_create читает ядро).
//!
//! Официально — Wallet Endpoints (прочитано 17.11.2026): «Deposit Address (supporting network)» GET
//! /api/v3/capital/deposit/address (https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/deposit-address-supporting-network,
//! SPOT_WITHDRAW_READ, вес 10; coin обязателен, network — нет); «Generate deposit address (supporting network)» POST
//! того же пути (https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/generate-deposit-address-supporting-network):
//! право «SPOT_WITHDRAW_WRITE», вес (IP) 1; coin и network обязательны (network — «deposit network»); ответ — address,
//! coin, memo, network; пример ответа — массив записей. Какое имя сети ждёт network (код или отображаемое, как
//! «BEP20(BSC)»), документация не говорит — передаётся то же имя, что и в GET (отображаемое из справочника сетей);
//! проверить первым созданием (docs/TASKS.md, П1). Повторное создание и предел числа адресов не описаны. Живого ответа
//! POST нет: тест выбора — на примере из документации (tests/data/mexc_doc_2026-11-17); живой ответ — в сыром журнале
//! сервера (<data-dir>/raw/mexc/*.jsonl.zst) при первом создании.

use reqwest::Method;
use serde_json::Value;

use crate::core::types::Asset;
use crate::gateway::addr_create::{Confirm, confirm};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::errors::Op;
use super::{num, rest};

/// Путь обоих методов (GET — адрес, POST — создание).
const PATH: &str = "/api/v3/capital/deposit/address";

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

/// Непустой address записи. Ошибок нет.
fn address(r: &Value) -> Option<String> {
    num::text_opt(r, "address").filter(|a| !a.trim().is_empty())
}

/// Что делает: Deposit Address GET (ссылка — в шапке) по монете и отображаемому имени сети.
/// Принимает: окружение, монету, отображаемое имя сети. Возвращает: ответ. Ошибки: связь, коды MEXC.
pub async fn list(ctx: &Ctx, asset: &Asset, display: &str) -> GatewayResult<Value> {
    let p = vec![
        ("coin", asset.ticker.clone()),
        ("network", display.to_string()),
    ];
    rest::signed(ctx, Method::GET, PATH, p, Op::Other).await
}

/// Что делает: создать адрес (Generate deposit address) и подтвердить его повторным GET (Б1, Н1 — см. шапку).
/// Принимает: окружение, монету, отображаемое имя сети (как в GET), код сети.
/// Возвращает: запись сети из GET (address, memo — memo проверяет вызывающий). После подтверждения — журнал
/// venue.deposit_address (Info, без адреса — как KuCoin, решение 208); расхождение — тот же журнал (Warn, без адресов).
/// Ошибки: отказ POST (код и сообщение MEXC дословно, с переданным network) или итог неизвестен — если и GET записи
/// не дал; InvalidAddress — POST принят, а GET адреса не дал, или адреса POST и GET разные; ошибка повторного GET.
pub async fn create(
    ctx: &Ctx,
    asset: &Asset,
    display: &str,
    network: &str,
) -> GatewayResult<Value> {
    let p = vec![
        ("coin", asset.ticker.clone()),
        ("network", display.to_string()),
    ];
    let posted = rest::signed(ctx, Method::POST, PATH, p, Op::Create)
        .await
        .map(|v| pick(&v, display, network).and_then(address))
        .map_err(|mut e| {
            e.message = format!(
                "MEXC: запрос создания адреса депозита {} (POST {PATH}, coin={}, network={display}) не удался: {}",
                asset.ticker, asset.ticker, e.message
            );
            e
        });
    let got = list(ctx, asset, display).await?;
    let rec = pick(&got, display, network);
    let listed = rec.and_then(address);
    let from_post = posted.as_ref().ok().and_then(|a| a.as_deref());
    let what = format!("MEXC {} в сети {display}", asset.ticker);
    match (confirm(from_post, listed.as_deref()), rec) {
        (Confirm::Use(_), Some(r)) => {
            let m = format!("Создан адрес депозита {what} — подтверждён повторным GET {PATH}");
            let _ = ctx
                .journal
                .write(Level::Info, "venue.deposit_address", Some(ID), m, None)
                .await;
            Ok(r.clone())
        }
        (Confirm::Mismatch, _) => {
            let m = format!(
                "Адрес депозита {what}: адрес из ответа POST {PATH} не совпал с адресом повторного GET — адрес не \
                 принят, сделки нет"
            );
            let _ = ctx
                .journal
                .write(
                    Level::Warn,
                    "venue.deposit_address",
                    Some(ID),
                    m.clone(),
                    None,
                )
                .await;
            Err(invalid(ctx, m))
        }
        _ => match posted {
            Err(e) => Err(e),
            Ok(_) => Err(invalid(
                ctx,
                format!(
                    "Адрес депозита {what}: POST {PATH} принят, но повторный GET записи сети с адресом не дал — адрес \
                     не принят, сделки нет"
                ),
            )),
        },
    }
}

/// Ошибка InvalidAddress с площадкой MEXC. Ошибок нет.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
