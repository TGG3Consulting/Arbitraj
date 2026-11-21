//! Файл: venues/bsc_dex/pools/history_store.rs
//! Что делает: история пулов V4 в SQLite (storage/venue_state.rs), чтобы после перезапуска читать только новые блоки:
//! по виду пула и адресу менеджера — сами события Initialize найденных пулов (из лога заново собирается PoolKey,
//! PoolId сверяется — discover_v4::parse) и последний прочитанный блок. Другой адрес менеджера — другой ключ:
//! история читается заново от блока развёртывания.
//! Кто вызывает: pools/history_v4.rs (загрузка, запись по кускам), pools/discover.rs (запись дочитанных блоков).
//! От чего зависит: ctx.rs, storage/venue_state.rs, history_ekubo.rs (разбор V4 и Ekubo), discover.rs (Found).
//! Решение 454: так же хранится история PoolInitialized ядра Ekubo.
//! Настройки: нет.
//!
//! Хранится лог как его вернул узел (execution-apis src/schemas/receipt.yaml): разбор тот же, что при чтении из сети.
//! Порядок записи: сначала пулы куска, потом последний блок — сбой между ними лишь заставит перечитать кусок.

use alloy::rpc::types::Log;
use alloy_primitives::Address;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::discover::Found;
use super::families::Family;
use super::history_ekubo;
use crate::storage::venue_state;

/// Вид записи пула (id — «вид:менеджер:PoolId»).
const POOL: &str = "v4_pool";
/// Вид записи последнего прочитанного блока (id — «вид:менеджер»).
const LAST: &str = "v4_last";

/// Ключ вида пула и менеджера. Ошибок нет.
fn base(f: &Family, manager: Address) -> String {
    format!("{}:{manager:#x}", f.id)
}

/// Сохранённая история: (последний прочитанный блок, пулы). None — для этого менеджера ничего не сохранено.
/// Ошибки: Internal — база не прочитана, запись не разобрана; BadResponse — лог не по ABI (как при чтении из сети).
pub async fn load(
    ctx: &Ctx,
    f: &'static Family,
    manager: Address,
) -> GatewayResult<Option<(u64, Vec<Found>)>> {
    let db = ctx.journal.db();
    let id = base(f, manager);
    let internal = |e: String| {
        ctx.err(
            ErrorCode::Internal,
            format!("{}: история V4 в SQLite: {e}", f.title),
        )
    };
    let last = venue_state::load(db, ctx.place.id, LAST, &id)
        .await
        .map_err(|e| internal(e.to_string()))?;
    let Some(last) = last else { return Ok(None) };
    let last: u64 = last
        .parse()
        .map_err(|_| internal(format!("блок «{last}» не число")))?;
    let rows = venue_state::list(db, ctx.place.id, POOL, &format!("{id}:"))
        .await
        .map_err(|e| internal(e.to_string()))?;
    let mut logs = Vec::with_capacity(rows.len());
    for (key, body) in rows {
        let log: Log = serde_json::from_str(&body).map_err(|e| internal(format!("{key}: {e}")))?;
        logs.push(log);
    }
    Ok(Some((last, history_ekubo::parse_all(ctx, f, &logs)?)))
}

/// Сохраняет кусок истории: события Initialize и последний прочитанный блок.
/// Принимает: вид пула, менеджер, логи куска (уже разобранные без ошибок), блок, до которого прочитано.
/// Ошибки: Internal — база (вызывающий пишет в журнал: в памяти история верна, после перезапуска кусок перечитается).
pub async fn save(
    ctx: &Ctx,
    f: &Family,
    manager: Address,
    logs: &[Log],
    last: u64,
) -> GatewayResult<()> {
    let db = ctx.journal.db();
    let id = base(f, manager);
    let internal = |e: String| {
        ctx.err(
            ErrorCode::Internal,
            format!("{}: история V4 не сохранена: {e}", f.title),
        )
    };
    let mut rows = Vec::with_capacity(logs.len());
    for log in logs {
        // Решение 454: у Ekubo PoolId — в данных события (history_ekubo::store_id).
        let pool_id = history_ekubo::store_id(log);
        let body = serde_json::to_string(log).map_err(|e| internal(e.to_string()))?;
        rows.push((format!("{id}:{pool_id:#x}"), body));
    }
    if !rows.is_empty() {
        venue_state::save_many(db, ctx.place.id, POOL, rows)
            .await
            .map_err(|e| internal(e.to_string()))?;
    }
    venue_state::save(db, ctx.place.id, LAST, &id, last.to_string())
        .await
        .map_err(|e| internal(e.to_string()))
}
