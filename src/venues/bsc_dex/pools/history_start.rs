//! Файл: venues/bsc_dex/pools/history_start.rs
//! Что делает: стартовая точка истории пулов V4: известное без узла (память подписки → SQLite) и, только при
//! разрешённом dex.history_scan, блок развёртывания менеджера (решение 121, C1 — выключатель до поиска блока).
//! Кто вызывает: pools/history_v4.rs.
//! От чего зависит: discover.rs (History, Scanned, Found), history_store.rs, deploy_block.rs, families.rs.
//! Настройки: нет своих (dex.history_scan проверяет history_v4.rs через history_gate.rs).

use alloy_primitives::Address;

use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::discover::{Found, History, Scanned};
use super::families::Family;
use super::listen::Pools;
use super::{deploy_block, history_store};

/// Откуда читать и что уже известно — без обращения к узлу: память (тот же менеджер) → SQLite.
/// Возвращает: (следующий блок для чтения — None, если истории нет и нужен блок развёртывания; известные пулы).
/// Ошибки: SQLite.
pub async fn start(
    p: &Pools,
    f: &'static Family,
    manager: Address,
    scanned: &Scanned,
) -> GatewayResult<(Option<u64>, Vec<Found>)> {
    // Память: дальше читает только эта загрузка («не догнала» — обычный поиск блоки не читает, без двойного чтения).
    if let Some(h) = scanned
        .inner
        .lock()
        .await
        .get_mut(f.id)
        .filter(|h| h.manager == manager)
    {
        h.caught_up = false;
        return Ok((Some(h.last + 1), h.pools.clone()));
    }
    let (last, pools) = match history_store::load(&p.ctx, f, manager).await? {
        Some((last, pools)) => {
            let msg = format!(
                "{}: из SQLite {} пулов, история прочитана до блока {last}",
                f.title,
                pools.len()
            );
            p.ctx.log(Level::Info, "dex.scan", msg, None).await;
            (Some(last), pools)
        }
        None => (None, Vec::new()),
    };
    let Some(last) = last else {
        return Ok((None, pools));
    };
    remember(scanned, f, manager, last, pools.clone()).await;
    Ok((Some(last + 1), pools))
}

/// Истории нет ни в памяти, ни в SQLite: блок развёртывания менеджера (deploy_block.rs, eth_getCode по старым
/// блокам). Вызывается только при разрешённом dex.history_scan. Возвращает: первый блок для чтения.
/// Ошибки: поиск блока развёртывания (узел, SQLite).
pub async fn first(
    p: &Pools,
    f: &'static Family,
    manager: Address,
    scanned: &Scanned,
) -> GatewayResult<u64> {
    let next = deploy_block::get(&p.ctx, f, manager).await?;
    remember(scanned, f, manager, next.saturating_sub(1), Vec::new()).await;
    Ok(next)
}

/// Запоминает в памяти подписки, до какого блока прочитана история вида. Ошибок нет.
async fn remember(
    scanned: &Scanned,
    f: &'static Family,
    manager: Address,
    last: u64,
    pools: Vec<Found>,
) {
    let h = History {
        manager,
        last,
        pools,
        caught_up: false,
    };
    scanned.inner.lock().await.insert(f.id, h);
}
