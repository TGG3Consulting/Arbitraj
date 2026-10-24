//! Файл: venues/bsc_dex/pools/history_v4.rs
//! Что делает: фоновая загрузка истории пулов V4 (Infinity CL, Infinity Bin, Uniswap V4) — подписка на рынок V2/V3
//! её не ждёт. По каждому включённому виду V4: память (если история уже есть) → SQLite (history_store.rs) →
//! иначе от блока развёртывания менеджера (deploy_block.rs); затем новые блоки кусками dex.logs_block_batch до
//! текущего блока сети. После каждого куска найденное сохраняется в SQLite; к подписке (attach.rs) найденное
//! подключается раз в dex.scan_progress_s (вместе со строкой хода) и в конце чтения — пулы V4 слушаются по мере
//! нахождения, без перезапуска слушателей V2/V3 и без подписки на каждый кусок.
//! Догнала сеть — история помечается «догнала»: дальше новые пулы V4 приходят событиями Initialize (created.rs).
//! Кто вызывает: pools/market.rs (задача подписки на рынок; при новой подписке прерывается — память и SQLite
//! остаются согласованными: блок сдвигается только после записи куска).
//! От чего зависит: ctx.rs, node/rpc.rs, discover.rs, discover_v4.rs, deploy_block.rs, history_store.rs, scan.rs,
//! attach.rs, created.rs (Watch), families.rs.
//! Необязательна (решение 58): пулы V4 и Infinity приходят из справочника индексатора с проверкой ключа у менеджера
//! или PositionManager; история нужна только для пулов Uniswap V4 без позиций через PositionManager.
//! Настройки: dex.pool_enabled, dex.pool_manager [DEX V4]; dex.logs_block_batch,
//! dex.scan_progress_s, dex.pools_per_sub, dex.history_scan (пауза из интерфейса — history_gate.rs) [bsc_dex].
//!
//! eth_blockNumber, eth_getLogs — https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth ;
//! Initialize — единственный источник списка пулов V4 (research/flow/bsc_v4.md, раздел 3).

use alloy_primitives::{Address, U64};
use futures_util::future::join_all;
use serde_json::json;

use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::check::recheck::Recheck;
use super::super::node::rpc;
use super::attach::attach;
use super::created::Watch;
use super::discover::{self, Found, History, Scanned, against, is_v4};
use super::families::{self, Family};
use super::listen::Pools;
use super::{deploy_block, discover_v4, history_gate, history_store, scan};

/// Загрузка всех включённых видов V4 параллельно. Ошибка вида — в журнал (остальные виды грузятся).
/// В конце, если подключены новые пулы, — «новые пулы» циклам крючков и сверки (check/recheck.rs):
/// до проверки пул с крючком B/C не торгуется. Ошибок нет.
pub async fn run(p: Pools, scanned: Scanned, watches: Vec<Watch>, wake: Recheck) {
    let mut jobs = Vec::new();
    for f in families::all(&p.ctx).into_iter().filter(|f| is_v4(f)) {
        if discover::enabled(&p.ctx, f).await {
            jobs.push(family(&p, f, &scanned, &watches));
        }
    }
    let mut attached = 0;
    for (f, r) in join_all(jobs).await {
        match r {
            Ok(n) => attached += n,
            Err(e) => {
                let msg = format!("{}: история пулов: {}", f.title, e.message);
                p.ctx.log(Level::Error, "dex.scan", msg, None).await;
            }
        }
    }
    if attached > 0 {
        wake.new_pools();
    }
}

/// Один вид пула: стартовая точка, подключение уже известного, дочитывание до сети.
/// Возвращает: (вид, итог). Ошибки: NotConfigured (адрес менеджера, настройки чтения), узел, SQLite.
async fn family(
    p: &Pools,
    f: &'static Family,
    scanned: &Scanned,
    watches: &[Watch],
) -> (&'static Family, GatewayResult<usize>) {
    (f, load(p, f, scanned, watches).await)
}

/// Тело family. Возвращает: сколько пулов подключено. Ошибки: как у family.
async fn load(
    p: &Pools,
    f: &'static Family,
    scanned: &Scanned,
    watches: &[Watch],
) -> GatewayResult<usize> {
    let ctx = &p.ctx;
    let manager = ctx.dex_addr("dex.pool_manager", f.id)?;
    let (mut next, known) = start(p, f, manager, scanned).await?;
    let mut attached = connect(p, watches, &known).await?;
    let what = format!("{}: история Initialize", f.title);
    let topics = json!([discover_v4::topic(f)]);
    let batch = scan::batch_size(ctx)?;
    loop {
        let head: U64 = rpc::call(ctx, "eth_blockNumber", json!([])).await?;
        let head: u64 = head.to();
        if next > head {
            if let Some(h) = scanned.inner.lock().await.get_mut(f.id) {
                h.caught_up = h.manager == manager;
            }
            return Ok(attached);
        }
        let mut prog = scan::Progress::new(ctx, &what, next, head)?;
        let mut events = 0;
        // Найденные пулы копятся и подключаются раз в dex.scan_progress_s (вместе со строкой хода) и в конце чтения:
        // каждое подключение — подписка на узле (платное сообщение), а история находит пулы почти в каждом куске.
        let mut pending: Vec<Found> = Vec::new();
        while next <= head {
            // Вид выключили на лету — чтение останавливается на следующем куске (место чтения сохранено в SQLite).
            if !history_gate::allowed(ctx, &what).await {
                return Ok(attached + connect(p, watches, &pending).await?);
            }
            if !discover::enabled(ctx, f).await {
                let msg = format!("{what}: вид пула выключен — чтение остановлено на блоке {next}");
                ctx.log(Level::Info, "dex.scan", msg, None).await;
                return Ok(attached + connect(p, watches, &pending).await?);
            }
            let end = head.min(next.saturating_add(batch - 1));
            let logs = scan::one(ctx, manager, &topics, next, end, &what).await?;
            let found = discover_v4::parse_all(ctx, f, &logs)?;
            if let Err(e) = history_store::save(ctx, f, manager, &logs, end).await {
                ctx.log(Level::Error, "dex.scan", e.message, None).await;
            }
            {
                let mut m = scanned.inner.lock().await;
                let Some(h) = m.get_mut(f.id).filter(|h| h.manager == manager) else {
                    return Ok(attached);
                };
                h.pools.extend(found.iter().cloned());
                h.last = end;
            }
            pending.extend(found);
            events += logs.len();
            next = end + 1;
            if prog.due(end) {
                let n = connect(p, watches, &std::mem::take(&mut pending)).await?;
                attached += n;
                prog.attached += n;
            }
            prog.step(ctx, end, events).await;
        }
        let n = connect(p, watches, &pending).await?;
        attached += n;
        prog.attached += n;
        prog.finish(ctx, events).await;
    }
}

/// Откуда читать и что уже известно: память (тот же менеджер) → SQLite → блок развёртывания.
/// Возвращает: (следующий блок для чтения, известные пулы). Ошибки: SQLite, поиск блока развёртывания.
async fn start(
    p: &Pools,
    f: &'static Family,
    manager: Address,
    scanned: &Scanned,
) -> GatewayResult<(u64, Vec<Found>)> {
    // Память: дальше читает только эта загрузка («не догнала» — обычный поиск блоки не читает, без двойного чтения).
    if let Some(h) = scanned
        .inner
        .lock()
        .await
        .get_mut(f.id)
        .filter(|h| h.manager == manager)
    {
        h.caught_up = false;
        return Ok((h.last + 1, h.pools.clone()));
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
    let next = match last {
        Some(b) => b + 1,
        None => deploy_block::get(&p.ctx, f, manager).await?,
    };
    let h = History {
        manager,
        last: next.saturating_sub(1),
        pools: pools.clone(),
        caught_up: false,
    };
    scanned.inner.lock().await.insert(f.id, h);
    Ok((next, pools))
}

/// Подключает пулы к каждому наблюдателю (USDT и промежуточные монеты) по его встречному токену.
/// Возвращает: сколько подключено. Ошибки: NotConfigured (dex.pools_per_sub).
async fn connect(p: &Pools, watches: &[Watch], pools: &[Found]) -> GatewayResult<usize> {
    let mut n = 0;
    for w in watches {
        let mine: Vec<Found> = pools.iter().filter_map(|x| against(x, w.usdt)).collect();
        n += attach(p, w, mine, false).await?;
    }
    Ok(n)
}
