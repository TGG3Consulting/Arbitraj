//! Файл: venues/bsc_dex/check/quoter_block.rs
//! Что делает: решение 261, часть 2 (Тигран 12.11.2026, ответ на вопрос 11: «(а) + сравнение с памятью») — блок
//! периодической сверки и свежее состояние пула на нём:
//! - блок сверки = голова H — ОДИН запрос eth_blockNumber в момент сверки пула;
//! - пул читается заново на этом же H тем же загрузчиком, что при старте (pools/load.rs refresh — все чтения вида пула
//!   идут на номер блока: резервы, slot0, liquidity, тики, бины, состояние крючка/оракула), в реестр НЕ пишется;
//!   наша математика считается по свежему состоянию, квотер — eth_call на тот же H (check/quoter_compare.rs). Так
//!   сверяется только математика, расхождение не может быть сменой блока;
//! - сравнение с памятью (memory_note): свежее состояние на H против состояния пула в памяти бота. Память легитимно
//!   отстаёт на 1–2 блока: события применяются сессией пачки асинхронно (pools/session.rs — из канала по одному;
//!   лента квитанций node/receipts.rs deliver ставит «последний РОЗДАННЫЙ блок», не «применённый»; у WebSocket отметки
//!   блока нет), а WebSocket-узел может быть впереди узла HTTP. Поэтому «память отстаёт от сети» пишется, только если
//!   верхнее состояние (PoolState) различается И последнее применённое событие пула (synced_block) не позже
//!   H − dex.quoter_memory_lag_blocks (не задана — сравнение не выполняется). Карты тиков и бинов не сравниваются:
//!   их охват зависит от момента чтения (dex.tick_words вокруг текущего тика). Это отдельная причина в журнал и на
//!   экран «Глубина», БЕЗ паузы и без алерта.
//!
//! Узел не отдал голову, состояние на H (−32000 «historical state … is not available» → StaleData) или любой сбой
//! чтения — «не сверено (блок)»: не ошибка математики, сторона не исключается и не паузится. Не задана настройка
//! чтения — отказ с её именем (сторона ждёт её, решение 75).
//!
//! Факты своего узла BSC 12.11.2026 (tests/data/node_limits_2026-11-12): состояние на номер блока — только H … H−127;
//! eth_getLogs — не больше 5000 блоков (больше не используется: eth_getLogs из сверки убран).
//! Кто вызывает: check/quoter_verify.rs (at_head — лениво, один раз на пул), check/quoter_one.rs (memory_note).
//! От чего зависит: node/rpc.rs, node/jsonrpc.rs, pools/load.rs (refresh), pools/registry.rs, tokens.rs.
//! Настройки: dex.quoter_memory_lag_blocks [bsc_dex] (необязательна); через pools/load.rs — настройки чтения пула.
//!
//! eth_blockNumber, eth_call(transaction, block) — execution-apis
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth

use alloy_primitives::U64;
use serde_json::json;
use serde_json::value::RawValue;

use crate::gateway::errors::ErrorCode;

use super::super::ctx::Ctx;
use super::super::node::{jsonrpc, rpc};
use super::super::pools::listen::Pools;
use super::super::pools::load;
use super::super::pools::registry::Entry;
use super::quoter_amount::Fail;

/// Начало причины «не сверено (блок)» — по ней сторона не исключается и не паузится (check/quoter_pause.rs).
pub const NOT_CHECKED: &str = "не сверено (блок)";
/// Настройка допуска отставания памяти, блоков (необязательна).
pub const MEMORY_LAG: &str = "dex.quoter_memory_lag_blocks";

/// Свежее состояние пула для сверки.
#[derive(Debug, Clone)]
pub enum Head {
    /// Пул прочитан на голове: блок H и запись со свежим состоянием (в реестр не записана).
    Ready { block: u64, entry: Box<Entry> },
    /// Голова или чтение на ней не удались — «не сверено (блок)» с причиной.
    NoBlock(String),
    /// Не задана настройка чтения пула — причина и её имя.
    Fail(Fail),
}

/// Номер блока из ответа eth_blockNumber (result, hex). Ошибки (текст): ответ не по виду.
pub fn head_of(raw: &RawValue) -> Result<u64, String> {
    jsonrpc::decode::<U64>(raw, rpc::SOURCE)
        .map(|n| n.to::<u64>())
        .map_err(|e| e.message)
}

/// Голова (один eth_blockNumber) и пул, прочитанный на ней. Принимает: пулы, запись из памяти. Ошибок нет: итог — Head.
pub async fn at_head(p: &Pools, e: &Entry) -> Head {
    let block = match rpc::call_raw(&p.ctx, "eth_blockNumber", json!([])).await {
        Ok(raw) => match head_of(&raw) {
            Ok(b) => b,
            Err(why) => return no_block(format!("eth_blockNumber: {why}")),
        },
        Err(err) => return no_block(format!("eth_blockNumber: {}", err.message)),
    };
    match load::refresh(&p.ctx, &p.tokens, e, block).await {
        Ok(entry) => Head::Ready {
            block,
            entry: Box::new(entry),
        },
        Err(err) if err.code == ErrorCode::NotConfigured => Head::Fail((err.message, err.setting)),
        Err(err) => no_block(format!("чтение пула на блоке {block}: {}", err.message)),
    }
}

/// «Не сверено (блок)» с причиной. Ошибок нет.
pub fn no_block(why: impl std::fmt::Display) -> Head {
    Head::NoBlock(format!("{NOT_CHECKED}: {why}"))
}

/// Сравнение памяти со свежим состоянием (чистая, тестируется на записанных событиях). Принимает: запись из памяти,
/// свежую на H, H, допуск отставания (блоков). Возвращает: причину «память отстаёт от сети» или None.
pub fn memory_lag(mem: &Entry, fresh: &Entry, head: u64, lag: u64) -> Option<String> {
    if mem.pool.state == fresh.pool.state || mem.synced_block > head.saturating_sub(lag) {
        return None;
    }
    Some(format!(
        "память отстаёт от сети: состояние пула в памяти (последнее событие — блок {}{}) не равно состоянию на блоке \
         {head}, отставание больше {MEMORY_LAG} = {lag}",
        mem.synced_block,
        mem.synced_log
            .map_or(String::new(), |i| format!(", logIndex {i}"))
    ))
}

/// Сравнение памяти по настройке: не задана (или задана неверно) — сравнение не выполняется (None). Ошибок нет.
pub fn memory_note(ctx: &Ctx, mem: &Entry, h: &Head) -> Option<String> {
    let Head::Ready { block, entry } = h else {
        return None;
    };
    let lag = ctx.int(MEMORY_LAG).ok()?;
    memory_lag(mem, entry, *block, lag)
}
