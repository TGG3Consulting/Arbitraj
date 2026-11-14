//! Файл: venues/bsc_dex/pools/fee_watch.rs
//! Что делает: решение 281 — события смены комиссии пулов V3-форка SquadSwap V3 в реальном времени. Комиссию `fee()`
//! пула задаёт менеджер (`poolManager.getFee(pool, defaultFee)`, SquadV3Pool.sol L136-L137); оператор менеджера меняет
//! её setFee / setFeeBatch / toggleCustomFee, менеджер пишет события SetCustomFee(address pool, uint24 fee) и
//! ToggleCustomFee(address pool, bool enabled) — поля НЕ индексированы (SquadV3PoolManager.sol L9, L11, L43-L66);
//! фабрика меняет менеджера changePoolManager → UpdatePoolManager(address indexed _manager) (SquadV3Factory.sol
//! L150-L152, ISquadV3Factory.sol L43). Подписка logs (тем же источником, что пулы: bsc.pool_events_source) на менеджера
//! из dex.v3_fee_manager (оба события) и на фабрику DEX (UpdatePoolManager). Событие менеджера → пул из данных, если он в
//! реестре — fee() пула перечитывается на узле и записывается в реестр (сигналы сразу считают по новой комиссии).
//! UpdatePoolManager → предупреждение в журнал (задать новый dex.v3_fee_manager) и перечтение fee() всех пулов этой
//! DEX в реестре. При открытии подписки poolManager() фабрики сверяется с настройкой — расхождение в журнал; после
//! подписки (в начале КАЖДОЙ сессии, и после обрыва) fee() всех пулов DEX перечитывается пачкой (fee_refresh.rs).
//! Лог менеджера не по ABI — Warn; venue.ws_reconnect_pause_ms не задана — ожидание настройки (settings_wait.rs).
//! Это ускорение: сделка всё равно считается по fee(), перечитанной на блоке верификации (fee_fresh.rs,
//! dex.v3_fee_source = pool_fee_each_read). Нет DEX с dex.v3_fee_manager — подписки нет.
//! Кто вызывает: pools/market.rs.
//! От чего зависит: ctx.rs, node/source.rs (Link), node/call.rs, families.rs, registry.rs, listen.rs (Pools),
//! fee_refresh.rs (перечтение fee()), abi_squad.rs (ABI событий, фильтры, пул из события), settings_wait.rs.
//! Настройки: dex.v3_fee_manager, dex.factory [DEX]; bsc.pool_events_source, venue.ws_reconnect_pause_ms [bsc_dex].
//!
//! Источник (verified, Sourcify partial_match, chain 56): SquadV3PoolManager 0x391eaa90f931c6330132efe6c73ebdf77d782ef5
//! (sha256 cbdbe8fd…cdc9), SquadV3Factory 0x10d8612d9d8269e322ab551c18a307cb4d6bc07b (sha256 064bd44a…fc06).

use std::time::Duration;

use alloy::sol_types::SolEvent;
use alloy_primitives::Address;
use serde_json::json;

use crate::core::types::PoolRef;
use crate::settings::live::Watch as Changes;
use crate::storage::journal::Level;

use super::super::node::call;
use super::super::node::source::Link;
use super::abi_squad::{ISquadV3Factory, filters, is_manager_event, pool_of};
use super::families::{self, Family, Proto};
use super::fee_refresh;
use super::listen::Pools;

/// DEX с менеджером комиссии: (вид, менеджер, фабрика) — только подключённые V3 с dex.v3_fee_manager. Ошибок нет.
fn sources(p: &Pools) -> Vec<(&'static Family, Address, Address)> {
    families::all(&p.ctx)
        .into_iter()
        .filter(|f| matches!(f.proto, Proto::UniswapV3 | Proto::PancakeV3))
        .filter(|f| families::classify(&p.ctx, f.id).is_ok())
        .filter_map(|f| {
            let m = p.ctx.dex_addr("dex.v3_fee_manager", f.id).ok()?;
            let fa = p.ctx.dex_addr("dex.factory", f.id).ok()?;
            Some((f, m, fa))
        })
        .collect()
}

/// Работает постоянно (как created.rs): изменение dex.* или источника событий — подписка пересобирается; нет DEX с
/// менеджером — ждём изменения настроек. Ошибок не возвращает: всё — в журнал.
pub async fn run(p: Pools) {
    let mut changes = Changes::new(
        p.ctx.settings.subscribe(),
        vec!["dex.".into(), "bsc.pool_events_source".into()],
    );
    loop {
        let list = sources(&p);
        if list.is_empty() {
            if changes.next_batch().await.is_none() {
                return;
            }
            continue;
        }
        let why = tokio::select! {
            why = session(&p, &list) => why,
            c = changes.next_batch() => match c { None => return, Some(_) => continue },
        };
        let msg = format!("Комиссии пулов (менеджер): {why}");
        p.ctx.log(Level::Warn, "venue.ws", msg, None).await;
        // Не задана — ждём её на лету (как пулы, listen.rs: settings_wait), а не завершаемся навсегда молча.
        let read = || p.ctx.int("venue.ws_reconnect_pause_ms");
        let what = "Комиссии пулов (менеджер)";
        let Some(ms) = super::super::settings_wait::until_ok(&p.ctx, what, "venue.ws", read).await else {
            return;
        };
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

/// Одна сессия до обрыва: сверка poolManager() фабрик, подписки, разбор событий. Возвращает причину конца.
async fn session(p: &Pools, list: &[(&'static Family, Address, Address)]) -> String {
    for (f, m, fa) in list {
        match call::view(&p.ctx, *fa, &ISquadV3Factory::poolManagerCall {}, "latest").await {
            Ok(now) if now == *m => {}
            Ok(now) => {
                let msg = format!(
                    "{}: poolManager() фабрики = {now:#x}, а dex.v3_fee_manager = {m:#x} — события комиссии \
                     слушаются не у того менеджера (решение 281)",
                    f.title
                );
                p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
            }
            Err(e) => return e.message,
        }
    }
    let conn = match Link::open(&p.ctx, &p.feed, &p.heads, &p.out, "fee_manager").await {
        Ok(c) => c,
        Err(e) => return e.message,
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    for filter in filters(list) {
        match conn.subscribe(json!(["logs", filter])).await {
            Ok(mut sub) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    while let Some(raw) = sub.rx.recv().await {
                        let _ = tx.send(raw.get().to_string());
                    }
                });
            }
            Err(e) => return e.message,
        }
    }
    drop(tx);
    // Ревью 281 (В2): подписка открыта — fee() всех пулов DEX заново (смена во время обрыва иначе потеряна).
    for (f, _, _) in list {
        fee_refresh::all(p, f, "перечтение после подписки").await;
    }
    while let Some(raw) = rx.recv().await {
        on_log(p, list, &raw).await;
    }
    conn.end_reason()
        .unwrap_or_else(|| "подписки закрыты".into())
}

/// Разбор одного лога: событие менеджера — перечтение fee() пула; UpdatePoolManager — предупреждение и перечтение всех
/// пулов DEX. Реорг (removed) — тоже перечтение: fee() на узле — истина. Ошибок нет: всё — в журнал.
async fn on_log(p: &Pools, list: &[(&'static Family, Address, Address)], raw: &str) {
    let Ok(log) = serde_json::from_str::<alloy::rpc::types::Log>(raw) else {
        p.ctx
            .log(
                Level::Warn,
                "dex.pool",
                "Комиссия пула: лог не разобран".to_string(),
                None,
            )
            .await;
        return;
    };
    let Some(t0) = log.topics().first().copied() else {
        return;
    };
    if let Some(pool) = pool_of(t0, log.data()) {
        fee_refresh::one(p, PoolRef::Contract(pool)).await;
        return;
    }
    // Ревью 281 (В2): событие менеджера не по ABI — не молча.
    if is_manager_event(t0) {
        let msg = format!("Комиссия пула: лог менеджера {:#x} не разобран по ABI (решение 281)", log.address());
        p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
        return;
    }
    if t0 != ISquadV3Factory::UpdatePoolManager::SIGNATURE_HASH {
        return;
    }
    let Some((f, m, _)) = list.iter().find(|(_, _, fa)| *fa == log.address()) else {
        return;
    };
    let new = ISquadV3Factory::UpdatePoolManager::decode_log_data(log.data())
        .map(|e| format!("{:#x}", e._manager))
        .unwrap_or_else(|_| "?".into());
    let msg = format!(
        "{}: фабрика сменила менеджера комиссии на {new} (было dex.v3_fee_manager = {m:#x}) — задайте новый \
         dex.v3_fee_manager; комиссии пулов перечитываются (решение 281)",
        f.title
    );
    p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
    fee_refresh::all(p, f, "смена менеджера").await;
}

