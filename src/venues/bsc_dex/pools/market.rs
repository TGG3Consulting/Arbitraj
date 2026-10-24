//! Файл: venues/bsc_dex/pools/market.rs
//! Что делает: подписка на рынок DEX (контракт subscribe_market): монеты и USDT из списка ядра, промежуточные монеты
//! (решение 52: пулы «монета — промежуточная» и «промежуточная — USDT»), пулы из справочника индексатора с проверкой на
//! узле (решение 58: index.rs, обновления — directory/refresh.rs), слушатели пачек пулов (dex.pools_per_sub),
//! наблюдатель новых пулов (события создания), головы сети; необязательная история V4 (history_v4.rs). Возвращает задачи — при новой
//! подписке старые останавливаются (без дублей соединений).
//! Кто вызывает: mod.rs (subscribe_market).
//! От чего зависит: ctx.rs, tokens.rs, node/heads.rs, node/subs.rs, check/cycle.rs, node/rpc.rs, discover.rs, listen.rs, created.rs,
//! events.rs, registry.rs, mids.rs, market_mids.rs, history_v4.rs, directory/refresh.rs (справочник пулов, решение 58).
//! Настройки: venue.quote_asset [bsc_dex] (тикер USDT в списке ядра), dex.pools_per_sub; через вызываемые — dex.*,
//! dex.intermediate_enabled, dex.intermediate_tokens.
//!
//! USDT: адрес берётся из списка монет ядра (адрес — из API биржи, CLAUDE.md: «адрес контракта у биржи совпадает
//! с адресом токена в пуле»); монета с тикером venue.quote_asset. Вопрос о стыке — Issue #7.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use alloy_primitives::{Address, U64};
use serde_json::json;
use tokio::task::JoinHandle;

use crate::core::types::{Asset, PoolRef};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::check::recheck::Recheck;
use super::super::check::{cycle, quoter_cycle, tax_cycle};
use super::super::node::{rpc, subs};
use super::created::{self, Watch};
use super::discover::{self, Found, Scanned};
use super::listen::{self, Pools};
use super::{history_v4, index, market_mids, mids};

/// Запускает всё для подписки на рынок.
/// Принимает: окружение пулов, память истории, сигнал «перепроверить» для циклов проверок, монеты ядра.
/// Возвращает: задачи. Ошибки: NotConfigured (venue.quote_asset, нет USDT в списке, dex.pools_per_sub), узел.
pub async fn subscribe(
    p: &Pools,
    scanned: &Scanned,
    wake: &Recheck,
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
    // Без опроса блоков нет свежести — подписка не начинается, ядро повторит её после задания настройки.
    super::super::node::heads::poll_every(&p.ctx)?;
    // Узел отвечает — иначе подписка не начинается (ядро повторит).
    let _: U64 = rpc::call(&p.ctx, "eth_blockNumber", json!([])).await?;
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
    // Справочник пулов от индексатора (решение 58): сохранённый — проверка на узле до первого поиска, обновления — задачей.
    let (refresh, dir_rx) = index::begin(&p.ctx, &all, usdt, &mid_list).await;
    tasks.push(refresh);
    let mut found = discover::find_all(&p.ctx, &all, usdt);
    let (via, watches) = market_mids::find(&p.ctx, &coins, &mid_list);
    let mut seen: HashSet<PoolRef> = found.iter().map(|f| f.reference.clone()).collect();
    found.extend(via.into_iter().filter(|f| seen.insert(f.reference.clone())));
    let is_mid = |a: &Address| mid_list.iter().any(|m| m.address == *a);
    p.reg.retain(|e| {
        (e.usdt == usdt && all.contains_key(&e.coin))
            || (is_mid(&e.usdt) && coins.contains_key(&e.coin) && e.coin != e.usdt)
    });
    let msg = format!(
        "DEX: пулов из справочника (проверены на узле) с {quote} и промежуточными монетами ({}): {} по {} монетам",
        mid_list.len(),
        found.len(),
        coins.len()
    );
    p.ctx.log(Level::Info, "dex.pool", msg, None).await;

    let known: HashSet<PoolRef> = found.iter().map(|f| f.reference.clone()).collect();
    // Стартовая загрузка V2/V3: первый проход налогов ждёт, пока каждая пачка закончит первую загрузку (pools/progress.rs).
    let start = chunks(&found, &all, per);
    p.signals.expect(start.len());
    for (name, chunk) in start {
        tasks.push(tokio::spawn(listen::run(
            p.clone(),
            name,
            chunk,
            true,
            None,
        )));
    }
    // Проверки (решения 30, 32): налоги (и промежуточных монет — у их шага тоже налог), крючки, сверка с квотером.
    tasks.push(tokio::spawn(tax_cycle::taxes(
        p.clone(),
        all.clone(),
        usdt,
        wake.taxes.clone(),
    )));
    tasks.push(tokio::spawn(cycle::hooks(p.clone(), wake.hooks.clone())));
    tasks.push(tokio::spawn(quoter_cycle::run(
        p.clone(),
        wake.quoter.clone(),
    )));
    let extra = Arc::new(Mutex::new(Vec::new()));
    let known = Arc::new(Mutex::new(known));
    let late = Arc::new(Mutex::new(HashMap::new()));
    // Новые пулы: с USDT (монеты и промежуточные) и с каждой промежуточной монетой.
    let watches: Vec<Watch> = std::iter::once((all, usdt))
        .chain(watches)
        .map(|(list, quote_token)| Watch {
            coins: list,
            usdt: quote_token,
            scanned: scanned.clone(),
            known: known.clone(),
            tasks: extra.clone(),
            late: late.clone(),
        })
        .collect();
    for w in &watches {
        tasks.push(tokio::spawn(created::run(p.clone(), w.clone())));
    }
    // Справочник обновил монеты — проверка их пулов и подключение новых (решение 58).
    tasks.push(tokio::spawn(index::watch(
        p.clone(),
        dir_rx,
        watches.clone(),
        wake.clone(),
    )));
    // История V4 — необязательна (dex.history_scan): пулы Uniswap V4 без позиций через PositionManager.
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

/// Пулы пачки с тикерами монет.
pub type Tickered = Vec<(Found, String)>;

/// Пулы одного вида (и менеджера V4) с тикерами: (ключ вида, пулы). Пулы без тикера монеты не слушаются. Ошибок нет.
pub fn by_family(
    found: &[Found],
    coins: &HashMap<Address, String>,
) -> Vec<(String, &'static str, Tickered)> {
    let mut groups: HashMap<(&'static str, Option<Address>), Tickered> = HashMap::new();
    for f in found {
        let manager = match &f.reference {
            PoolRef::Singleton { manager, .. } => Some(*manager),
            PoolRef::Contract(_) => None,
        };
        // Пулы найдены по монетам подписки, у каждой есть тикер; без тикера пул не слушается.
        if let Some(t) = coins.get(&f.coin) {
            groups
                .entry((f.family.id, manager))
                .or_default()
                .push((f.clone(), t.clone()));
        }
    }
    groups
        .into_iter()
        .map(|((family, manager), list)| (format!("{family}:{manager:?}"), family, list))
        .collect()
}

/// Пачки для подписки: (имя, пулы с тикерами), не больше `per` пулов в пачке. Ошибок нет.
pub fn chunks(
    found: &[Found],
    coins: &HashMap<Address, String>,
    per: usize,
) -> Vec<(String, Vec<(Found, String)>)> {
    let mut out = Vec::new();
    for (_, family, list) in by_family(found, coins) {
        for (i, part) in list.chunks(per).enumerate() {
            out.push((format!("{family}-{i}"), part.to_vec()));
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
