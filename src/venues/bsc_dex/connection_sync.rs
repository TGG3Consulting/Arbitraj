//! Файл: venues/bsc_dex/connection_sync.rs
//! Что делает: часть «Проверить подключение» DEX — синхронизация узла (решение 121, часть A3): eth_syncing = false
//! (иначе «узел догоняет сеть: блок X из Y» и что ещё строится — индекс транзакций, индекс состояния) и возраст головы
//! (часы сервера − время последнего блока в миллисекундах — timestamp и доля секунды из mixHash, решение 259; при
//! заданном feed.max_block_age_ms — сверка с ним).
//! Номер у догоняющего узла растёт каждую секунду, и проверка «есть блок» его не ловит — эта ловит.
//! Кто вызывает: connection.rs (check).
//! От чего зависит: ctx.rs, node/rpc.rs, node/head_age.rs.
//! Настройки: feed.max_block_age_ms [bsc_dex] (необязательная: не задана — возраст только показывается).
//!
//! Источники: eth_syncing — execution-apis
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/client.yaml
//! (false или объект startingBlock/currentBlock/highestBlock); поля и условие «готово» узла geth-bsc v1.8.1 —
//! internal/ethapi/api.go:166-195 (EthereumAPI.Syncing: false, только если SyncProgress.Done()), interfaces.go:148-153
//! (Done: currentBlock ≥ highestBlock и txIndexRemainingBlocks, stateIndexRemaining, trienodeIndexRemaining равны 0) —
//! https://github.com/bnb-chain/bsc/tree/v1.8.1 ; eth_getBlockByNumber «latest», поле timestamp (секунды) —
//! execution-apis src/schemas/block.yaml.

use alloy_primitives::{B256, U64};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ctx::Ctx;
use super::node::{head_age, rpc};

/// Поля number, timestamp и mixHash блока «latest» (доля секунды — mixHash, BEP-520, node/block_ms.rs).
#[derive(Deserialize)]
struct RawBlock {
    number: U64,
    timestamp: U64,
    #[serde(rename = "mixHash", default)]
    mix_hash: Option<B256>,
}

/// Что делает: синхронизация узла и возраст головы.
/// Принимает: окружение. Возвращает: описание («синхронизирован, блок N создан K с назад») или текст причины.
/// Ошибки (текстом): узел недоступен; узел догоняет сеть; блок старше feed.max_block_age_ms; ответ не по формату.
pub async fn check(ctx: &Ctx) -> Result<String, String> {
    let syncing: Value = rpc::call(ctx, "eth_syncing", json!([]))
        .await
        .map_err(|e| e.message)?;
    if syncing != Value::Bool(false) {
        return Err(behind(&syncing));
    }
    let block: Option<RawBlock> = rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false]))
        .await
        .map_err(|e| e.message)?;
    let b = block.ok_or("узел не отдал последний блок (eth_getBlockByNumber latest = null)")?;
    let number = b.number.to::<u64>();
    let t = super::node::block_ms::millis(b.timestamp.to::<u64>(), b.mix_hash);
    let age = head_age::age_ms(t);
    let secs = rust_decimal::Decimal::new(age, 3).round_dp(1);
    let shown = format!("блок {number} создан {secs} с назад");
    match head_age::max_age_ms(ctx).map_err(|e| e.message)? {
        Some(max) if age > max as i64 => Err(format!(
            "eth_syncing = false, но {shown} — больше {} ({max} мс): узел отстаёт от сети (или часы сервера \
             неверны — проверить NTP)",
            head_age::SETTING
        )),
        Some(max) => Ok(format!(
            "синхронизирован (eth_syncing = false), {shown}, порог {max} мс"
        )),
        None => Ok(format!(
            "синхронизирован (eth_syncing = false), {shown}; {} не задан — возраст головы в работе не проверяется",
            head_age::SETTING
        )),
    }
}

/// Что делает: текст «узел догоняет сеть» по объекту eth_syncing.
/// Принимает: ответ eth_syncing (не false). Возвращает: текст причины с блоками и остатком индексов. Ошибок нет.
fn behind(v: &Value) -> String {
    // Числа в ответе — hex-строки (hexutil.Uint64); нет поля или не число — показывается «?».
    let num = |k: &str| -> String {
        v.get(k)
            .and_then(Value::as_str)
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .map_or_else(|| "?".to_string(), |n| n.to_string())
    };
    if !v.is_object() {
        return format!("непонятный ответ eth_syncing: {v}");
    }
    format!(
        "узел догоняет сеть: блок {} из {}; осталось проиндексировать: транзакции — {} блоков, состояние — {}, узлы \
         дерева — {}. Торговать нельзя, пока eth_syncing не станет false",
        num("currentBlock"),
        num("highestBlock"),
        num("txIndexRemainingBlocks"),
        num("stateIndexRemaining"),
        num("trienodeIndexRemaining"),
    )
}
