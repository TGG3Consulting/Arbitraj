//! Файл: venues/bsc_dex/pools/market.rs
//! Что делает: подписка на рынок DEX (контракт subscribe_market): монеты и USDT из списка ядра, поиск пулов,
//! слушатели пачек пулов (dex.pools_per_sub), наблюдатель новых пулов, головы сети. Возвращает задачи — при новой
//! подписке старые останавливаются (без дублей соединений).
//! Кто вызывает: mod.rs (subscribe_market).
//! От чего зависит: ctx.rs, tokens.rs, node/heads.rs, node/subs.rs, node/rpc.rs, discover.rs, listen.rs, created.rs,
//! events.rs, registry.rs.
//! Настройки: venue.quote_asset [bsc_dex] (тикер USDT в списке ядра), dex.pools_per_sub; через вызываемые — dex.*.
//!
//! USDT: адрес берётся из списка монет ядра (адрес — из API биржи, CLAUDE.md: «адрес контракта у биржи совпадает
//! с адресом токена в пуле»); монета с тикером venue.quote_asset. Вопрос о стыке — Issue #7.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use alloy_primitives::{Address, U64};
use serde_json::json;
use tokio::task::JoinHandle;

use crate::core::types::{Asset, PoolRef};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::node::heads::Heads;
use super::super::node::{rpc, subs};
use super::created::{self, Watch};
use super::discover::{self, Found, Scanned};
use super::events;
use super::listen::{self, Pools};

/// Запускает всё для подписки на рынок.
/// Принимает: окружение пулов, головы сети, память истории, монеты ядра.
/// Возвращает: задачи. Ошибки: NotConfigured (venue.quote_asset, нет USDT в списке, dex.pools_per_sub), узел.
pub async fn subscribe(
    p: &Pools,
    heads: &Heads,
    scanned: &Scanned,
    assets: Vec<Asset>,
) -> GatewayResult<Vec<JoinHandle<()>>> {
    p.tokens.set(&assets);
    let quote = p.ctx.text("venue.quote_asset")?;
    let usdt = assets
        .iter()
        .find(|a| a.ticker == quote)
        .and_then(|a| a.bsc_contract)
        .ok_or_else(|| {
            p.ctx.err(
                ErrorCode::NotConfigured,
                format!("В списке монет ядра нет {quote} (venue.quote_asset) с адресом в BSC — пулы искать не с чем"),
            )
        })?;
    let coins: HashMap<Address, String> = assets
        .iter()
        .filter(|a| a.ticker != quote)
        .filter_map(|a| a.bsc_contract.map(|c| (c, a.ticker.clone())))
        .collect();
    let per = subs::per_sub(&p.ctx)?;
    let head: U64 = rpc::call(&p.ctx, "eth_blockNumber", json!([])).await?;
    let mut tasks = vec![tokio::spawn(heads.clone().run(p.ctx.clone()))];

    let found = discover::find_all(&p.ctx, &coins, usdt, head.to(), scanned).await;
    p.reg
        .retain(|e| coins.contains_key(&e.coin) && e.usdt == usdt);
    let msg = format!(
        "DEX: найдено пулов с {quote}: {} по {} монетам",
        found.len(),
        coins.len()
    );
    p.ctx.log(Level::Info, "dex.pool", msg, None).await;

    let known: HashSet<PoolRef> = found.iter().map(|f| f.reference.clone()).collect();
    for (name, chunk, filter) in chunks(&found, &coins, per) {
        tasks.push(tokio::spawn(listen::run(p.clone(), name, chunk, filter)));
    }
    let extra = Arc::new(Mutex::new(Vec::new()));
    let w = Watch {
        coins,
        usdt,
        scanned: scanned.clone(),
        known: Arc::new(Mutex::new(known)),
        tasks: extra.clone(),
    };
    tasks.push(tokio::spawn(created::run(p.clone(), w)));
    // Задачи слушателей новых пулов живут в общем списке created.rs; при новой подписке их останавливает этот
    // сборщик (он прерывается вместе с остальными задачами и при этом гасит своих).
    tasks.push(tokio::spawn(Reaper(extra).wait()));
    Ok(tasks)
}

/// Пачка для подписки: имя, пулы с тикерами, фильтр.
type Chunk = (String, Vec<(Found, String)>, serde_json::Value);

/// Пачки пулов одного вида для подписки: (имя, пулы с тикерами, фильтр).
fn chunks(found: &[Found], coins: &HashMap<Address, String>, per: usize) -> Vec<Chunk> {
    let mut by_family: HashMap<(&str, Option<Address>), Vec<&Found>> = HashMap::new();
    for f in found {
        let manager = match &f.reference {
            PoolRef::Singleton { manager, .. } => Some(*manager),
            PoolRef::Contract(_) => None,
        };
        by_family.entry((f.family.id, manager)).or_default().push(f);
    }
    let mut out = Vec::new();
    for ((family, manager), list) in by_family {
        for (i, part) in list.chunks(per).enumerate() {
            let topic0 = events::topics(part[0].family);
            let filter = match manager {
                None => {
                    let addrs: Vec<Address> = part
                        .iter()
                        .filter_map(|f| match f.reference {
                            PoolRef::Contract(a) => Some(a),
                            PoolRef::Singleton { .. } => None,
                        })
                        .collect();
                    subs::by_address(&addrs, &topic0)
                }
                Some(m) => {
                    let ids: Vec<_> = part
                        .iter()
                        .filter_map(|f| match f.reference {
                            PoolRef::Singleton { id, .. } => Some(id),
                            PoolRef::Contract(_) => None,
                        })
                        .collect();
                    subs::by_pool_id(m, &ids, &topic0)
                }
            };
            let with_tickers = part
                .iter()
                .map(|f| {
                    (
                        (*f).clone(),
                        coins.get(&f.coin).cloned().unwrap_or_default(),
                    )
                })
                .collect();
            out.push((format!("{family}-{i}"), with_tickers, filter));
        }
    }
    out
}

/// Сборщик задач слушателей новых пулов: при остановке гасит их.
struct Reaper(Arc<Mutex<Vec<JoinHandle<()>>>>);

impl Reaper {
    /// Ждёт вечно; задачу прерывают при новой подписке — тогда срабатывает Drop.
    async fn wait(self) {
        std::future::pending::<()>().await;
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        for h in self.0.lock().unwrap_or_else(|p| p.into_inner()).drain(..) {
            h.abort();
        }
    }
}
