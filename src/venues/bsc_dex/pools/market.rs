//! Файл: venues/bsc_dex/pools/market.rs
//! Что делает: подписка на рынок DEX (контракт subscribe_market): монеты и USDT из списка ядра, промежуточные монеты
//! (решение 52: пулы «монета — промежуточная» и «промежуточная — USDT»), поиск пулов,
//! слушатели пачек пулов (dex.pools_per_sub), наблюдатель новых пулов, головы сети; пулы V4 — фоновая загрузка истории
//! (history_v4.rs), подписка её не ждёт. Возвращает задачи — при новой
//! подписке старые останавливаются (без дублей соединений).
//! Кто вызывает: mod.rs (subscribe_market).
//! От чего зависит: ctx.rs, tokens.rs, node/heads.rs, node/subs.rs, check/cycle.rs, node/rpc.rs, discover.rs, listen.rs, created.rs,
//! events.rs, registry.rs, mids.rs, market_mids.rs, history_v4.rs.
//! Настройки: venue.quote_asset [bsc_dex] (тикер USDT в списке ядра), dex.pools_per_sub; через вызываемые — dex.*,
//! dex.intermediate_enabled, dex.intermediate_tokens.
//!
//! USDT: адрес берётся из списка монет ядра (адрес — из API биржи, CLAUDE.md: «адрес контракта у биржи совпадает
//! с адресом токена в пуле»); монета с тикером venue.quote_asset. Вопрос о стыке — Issue #7.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use alloy_primitives::{Address, U64};
use serde_json::json;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::core::types::{Asset, PoolRef};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::check::{cycle, quoter_cycle, tax_cycle};
use super::super::node::{rpc, subs};
use super::created::{self, Watch};
use super::discover::{self, Found, Scanned};
use super::listen::{self, Pools};
use super::{events, history_v4, market_mids, mids};

/// Запускает всё для подписки на рынок.
/// Принимает: окружение пулов, память истории, сигнал «перепроверить» для циклов проверок, монеты ядра.
/// Возвращает: задачи. Ошибки: NotConfigured (venue.quote_asset, нет USDT в списке, dex.pools_per_sub), узел.
pub async fn subscribe(
    p: &Pools,
    scanned: &Scanned,
    wake: &Arc<Notify>,
    assets: Vec<Asset>,
) -> GatewayResult<Vec<JoinHandle<()>>> {
    p.tokens.set(&assets);
    let quote = p.ctx.text("venue.quote_asset")?;
    let usdt = p.tokens.usdt(&p.ctx)?;
    let coins: HashMap<Address, String> = assets
        .iter()
        .filter(|a| a.ticker != quote)
        .filter_map(|a| a.bsc_contract.map(|c| (c, a.ticker.clone())))
        .collect();
    let per = subs::per_sub(&p.ctx)?;
    let head: U64 = rpc::call(&p.ctx, "eth_blockNumber", json!([])).await?;
    let mut tasks = vec![tokio::spawn(p.heads.clone().run(p.ctx.clone()))];

    // Промежуточные монеты (решение 52): выключено — пусто; не задано — в журнал, только USDT.
    let mid_list = match mids::read(&p.ctx, &p.tokens).await {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("Промежуточные монеты не используются: {}", e.message);
            p.ctx.log(Level::Warn, "dex.intermediate", msg, None).await;
            Vec::new()
        }
    };
    p.mids.set(mid_list.clone());
    let all = market_mids::with_mids(&coins, &mid_list);
    let mut found = discover::find_all(&p.ctx, &all, usdt, head.to(), scanned, false).await;
    let (via, watches) = market_mids::find(&p.ctx, &coins, &mid_list, head.to(), scanned).await;
    let mut seen: HashSet<PoolRef> = found.iter().map(|f| f.reference.clone()).collect();
    found.extend(via.into_iter().filter(|f| seen.insert(f.reference.clone())));
    let is_mid = |a: &Address| mid_list.iter().any(|m| m.address == *a);
    p.reg.retain(|e| {
        (e.usdt == usdt && all.contains_key(&e.coin))
            || (is_mid(&e.usdt) && coins.contains_key(&e.coin) && e.coin != e.usdt)
    });
    let msg = format!(
        "DEX: найдено пулов V2/V3 с {quote} и промежуточными монетами ({}): {} по {} монетам; V4 — загружается в фоне",
        mid_list.len(),
        found.len(),
        coins.len()
    );
    p.ctx.log(Level::Info, "dex.pool", msg, None).await;

    let known: HashSet<PoolRef> = found.iter().map(|f| f.reference.clone()).collect();
    // Стартовая загрузка V2/V3: первый проход налогов ждёт, пока каждая пачка закончит первую загрузку (pools/progress.rs).
    let start = chunks(&found, &all, per);
    p.signals.expect(start.len());
    for (name, chunk, filter) in start {
        tasks.push(tokio::spawn(listen::run(
            p.clone(),
            name,
            chunk,
            filter,
            true,
        )));
    }
    // Проверки (решения 30, 32): налоги (и промежуточных монет — у их шага тоже налог), крючки, сверка с квотером.
    tasks.push(tokio::spawn(tax_cycle::taxes(
        p.clone(),
        all.clone(),
        usdt,
        wake.clone(),
    )));
    tasks.push(tokio::spawn(cycle::hooks(p.clone(), wake.clone())));
    tasks.push(tokio::spawn(quoter_cycle::run(p.clone(), wake.clone())));
    let extra = Arc::new(Mutex::new(Vec::new()));
    let known = Arc::new(Mutex::new(known));
    // Новые пулы: с USDT (монеты и промежуточные) и с каждой промежуточной монетой.
    let watches: Vec<Watch> = std::iter::once((all, usdt))
        .chain(watches)
        .map(|(list, quote_token)| Watch {
            coins: list,
            usdt: quote_token,
            scanned: scanned.clone(),
            known: known.clone(),
            tasks: extra.clone(),
        })
        .collect();
    for w in &watches {
        tasks.push(tokio::spawn(created::run(p.clone(), w.clone())));
    }
    // Пулы V4 — в фоне: подписка V2/V3 уже работает, пулы V4 подключаются по мере загрузки истории.
    tasks.push(tokio::spawn(history_v4::run(
        p.clone(),
        scanned.clone(),
        watches,
        wake.clone(),
    )));
    // Задачи слушателей новых пулов живут в общем списке created.rs; при новой подписке их останавливает этот
    // сборщик (он прерывается вместе с остальными задачами и при этом гасит своих).
    tasks.push(tokio::spawn(Reaper(extra).wait()));
    Ok(tasks)
}

/// Пачка для подписки: имя, пулы с тикерами, фильтр.
pub type Chunk = (String, Vec<(Found, String)>, serde_json::Value);

/// Пачки пулов одного вида для подписки: (имя, пулы с тикерами, фильтр).
pub fn chunks(found: &[Found], coins: &HashMap<Address, String>, per: usize) -> Vec<Chunk> {
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
            // Пулы найдены по монетам подписки, у каждой есть тикер; без тикера пул не слушается.
            let with_tickers = part
                .iter()
                .filter_map(|f| coins.get(&f.coin).map(|t| ((*f).clone(), t.clone())))
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
