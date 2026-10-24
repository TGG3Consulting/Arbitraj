//! Файл: venues/bsc_dex/pools/created.rs
//! Что делает: новые пулы «монета — USDT / промежуточная» в реальном времени, между обновлениями справочника
//! индексатора (решение 58). Подписка на события создания со встречным токеном в индексированном поле (PairCreated V2,
//! PoolCreated V3 своих фабрик, Initialize V4 своих менеджеров — только подключённых DEX); событие с монетой подписки
//! (адрес монеты в индексированном поле) — пул из самого события, проверка на узле (verify.rs) и подключение к
//! подписке (attach.rs) — created_log.rs. Пулы чужих монет не трогаются.
//! Кто вызывает: pools/market.rs.
//! От чего зависит: ctx.rs, node/ws.rs, created_log.rs, discover.rs (Scanned), listen.rs.
//! Настройки: dex.factory, dex.pool_manager [DEX], venue.ws_reconnect_pause_ms [bsc_dex].
//!
//! События создания и фильтр по USDT — research/flow/bsc_dex.md (схема п. 2), research/flow/bsc_v4.md (схема п. 2).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy_primitives::{Address, B256};
use serde_json::{Value, json};

use crate::core::types::PoolRef;
use crate::storage::journal::Level;

use super::super::node::ws::Conn;
use super::created_log;
use super::discover::Scanned;
use super::listen::Pools;

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
    /// Пачки найденных позже пулов по виду (attach.rs): общие для всех наблюдателей.
    pub late: super::attach::Late,
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

/// Одна сессия до обрыва: подписки на события создания своих фабрик и менеджеров, каждое событие с монетой подписки —
/// пул проверяется на узле и подключается (created_log.rs). Возвращает причину конца.
async fn session(p: &Pools, w: &Watch) -> String {
    let conn = match Conn::open(&p.ctx, "created").await {
        Ok(c) => c,
        Err(e) => return e.message,
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
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
                            let _ = tx.send(raw.get().to_string());
                        }
                    }
                });
            }
            Err(e) => return e.message,
        }
    }
    drop(tx);
    while let Some(raw) = rx.recv().await {
        if let Err(why) = created_log::on_log(p, w, &raw).await {
            let msg = format!("Новый пул: {why}");
            p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
        }
    }
    conn.end_reason()
        .unwrap_or_else(|| "подписки закрыты".into())
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

/// Фильтры событий создания с USDT в token0/currency0 или token1/currency1 по подключённым DEX (families::classify):
/// V2/V3 — фабрика (dex.factory), V4 — менеджер (dex.pool_manager). Ошибок нет.
async fn filters(p: &Pools, usdt: Address) -> Vec<Value> {
    let u = B256::left_padding_from(usdt.as_slice());
    let mut out = Vec::new();
    for (addr, t0, v4, _) in created_log::sources(&p.ctx) {
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
