//! Файл: venues/bsc_dex/pools/created.rs
//! Что делает: новые пулы «монета/USDT» в реальном времени. Подписка на события создания с USDT в индексированном
//! поле (PairCreated V2, PoolCreated V3, Initialize V4); событие с одной из монет подписки (адрес монеты в
//! индексированном поле события) — сигнал повторить поиск (discover.rs: V2/V3 — вызовы фабрики, V4 — только новые
//! блоки истории) и запустить слушателя для пулов, которых ещё нет. Пулы чужих монет поиск не запускают.
//! Сразу после подписки — то же дочитывание: пулы, созданные между первым поиском и подпиской, не теряются.
//! Кто вызывает: pools/market.rs.
//! От чего зависит: ctx.rs, node/ws.rs, node/rpc.rs, discover.rs, listen.rs, families.rs, abi_v2v3.rs, abi_v4.rs.
//! Настройки: dex.factory, dex.pool_manager [вид пула], venue.ws_reconnect_pause_ms [bsc_dex].
//!
//! События создания и фильтр по USDT — research/flow/bsc_dex.md (схема п. 2), research/flow/bsc_v4.md (схема п. 2).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, U64};
use serde_json::{Value, json};

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::node::{rpc, subs, ws::Conn};
use super::abi_v2v3::{IV2Factory, IV3Factory};
use super::abi_v4::{IInfinityBin, IInfinityCl, IUniswapV4};
use super::discover::{self, Found, Scanned};
use super::families::{FAMILIES, Proto};
use super::listen::{self, Pools};

/// Что знает наблюдатель за новыми пулами.
#[derive(Clone)]
pub struct Watch {
    pub coins: HashMap<Address, String>,
    pub usdt: Address,
    pub scanned: Scanned,
    /// Пулы, у которых уже есть слушатель.
    pub known: Arc<Mutex<HashSet<PoolRef>>>,
    /// Задачи слушателей новых пулов (останавливаются вместе с подпиской на рынок).
    pub tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

/// Работает постоянно: подписка на события создания, дочитывание, слушатели новых пулов.
/// Ошибок не возвращает: всё — в журнал.
pub async fn run(p: Pools, w: Watch) {
    loop {
        let why = session(&p, &w).await;
        p.ctx
            .log(Level::Warn, "venue.ws", format!("Новые пулы: {why}"), None)
            .await;
        match p.ctx.int("venue.ws_reconnect_pause_ms") {
            Ok(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            Err(_) => return,
        }
    }
}

/// Одна сессия до обрыва. Возвращает причину конца.
async fn session(p: &Pools, w: &Watch) -> String {
    let conn = match Conn::open(&p.ctx, "created").await {
        Ok(c) => c,
        Err(e) => return e.message,
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mine: Arc<HashSet<B256>> = Arc::new(
        w.coins
            .keys()
            .map(|a| B256::left_padding_from(a.as_slice()))
            .collect(),
    );
    for filter in filters(p, w.usdt).await {
        match conn.subscribe(json!(["logs", filter])).await {
            Ok(mut sub) => {
                let tx = tx.clone();
                let mine = mine.clone();
                tokio::spawn(async move {
                    while let Some(raw) = sub.rx.recv().await {
                        if relevant(raw.get(), &mine) {
                            let _ = tx.send(());
                        }
                    }
                });
            }
            Err(e) => return e.message,
        }
    }
    drop(tx);
    if let Err(e) = rescan(p, w).await {
        return e.message;
    }
    while rx.recv().await.is_some() {
        if let Err(e) = rescan(p, w).await {
            return e.message;
        }
    }
    conn.end_reason()
        .unwrap_or_else(|| "подписки закрыты".into())
}

/// Дочитывает историю до текущего блока и запускает слушателей пулов, которых ещё нет.
async fn rescan(p: &Pools, w: &Watch) -> GatewayResult<()> {
    let head: U64 = rpc::call(&p.ctx, "eth_blockNumber", json!([])).await?;
    let found = discover::find_all(&p.ctx, &w.coins, w.usdt, head.to(), &w.scanned).await;
    let fresh: Vec<Found> = {
        let mut known = w.known.lock().unwrap_or_else(|x| x.into_inner());
        found
            .into_iter()
            .filter(|f| known.insert(f.reference.clone()))
            .collect()
    };
    for f in fresh {
        // Пулы ищутся только по монетам подписки — монета без тикера значит, что подписка сменилась: пропускаем.
        let Some(ticker) = w.coins.get(&f.coin).cloned() else {
            continue;
        };
        p.ctx
            .log(
                Level::Info,
                "dex.pool",
                format!("Новый пул {} {ticker}: {:?}", f.family.title, f.reference),
                None,
            )
            .await;
        let filter = match &f.reference {
            PoolRef::Contract(a) => subs::by_address(&[*a], &super::events::topics(f.family)),
            PoolRef::Singleton { manager, id } => {
                subs::by_pool_id(*manager, &[*id], &super::events::topics(f.family))
            }
        };
        let name = format!("{}-new", f.family.id);
        let h = tokio::spawn(listen::run(p.clone(), name, vec![(f, ticker)], filter));
        w.tasks.lock().unwrap_or_else(|x| x.into_inner()).push(h);
    }
    Ok(())
}

/// Касается ли событие создания монет подписки: адрес монеты — в одном из индексированных полей (topics[1..]).
/// Принимает: сырой лог, адреса монет (32 байта). Возвращает: да/нет; лог не разобран — да (поиск безопасен,
/// пропуск — нет). Ошибок нет.
pub fn relevant(raw: &str, mine: &HashSet<B256>) -> bool {
    match serde_json::from_str::<alloy::rpc::types::Log>(raw) {
        Ok(log) => log.topics().iter().skip(1).any(|t| mine.contains(t)),
        Err(_) => true,
    }
}

/// Фильтры событий создания с USDT в token0/currency0 или token1/currency1 по включённым видам.
async fn filters(p: &Pools, usdt: Address) -> Vec<Value> {
    let u = B256::left_padding_from(usdt.as_slice());
    let mut out = Vec::new();
    for f in FAMILIES {
        if !discover::enabled(&p.ctx, f).await {
            continue;
        }
        let (setting, t0, v4) = match f.proto {
            Proto::V2 => (
                "dex.factory",
                IV2Factory::PairCreated::SIGNATURE_HASH,
                false,
            ),
            Proto::PancakeV3 | Proto::UniswapV3 => (
                "dex.factory",
                IV3Factory::PoolCreated::SIGNATURE_HASH,
                false,
            ),
            Proto::InfinityCl => (
                "dex.pool_manager",
                IInfinityCl::Initialize::SIGNATURE_HASH,
                true,
            ),
            Proto::InfinityBin => (
                "dex.pool_manager",
                IInfinityBin::Initialize::SIGNATURE_HASH,
                true,
            ),
            Proto::UniswapV4 => (
                "dex.pool_manager",
                IUniswapV4::Initialize::SIGNATURE_HASH,
                true,
            ),
        };
        let Ok(addr) = p.ctx.dex_addr(setting, f.id) else {
            continue;
        };
        // V2/V3: токены — topics[1], topics[2]; V4: PoolId — topics[1], валюты — topics[2], topics[3].
        let (a, b) = if v4 {
            (
                json!([t0, Value::Null, u]),
                json!([t0, Value::Null, Value::Null, u]),
            )
        } else {
            (json!([t0, u]), json!([t0, Value::Null, u]))
        };
        out.push(json!({ "address": addr, "topics": a }));
        out.push(json!({ "address": addr, "topics": b }));
    }
    out
}
