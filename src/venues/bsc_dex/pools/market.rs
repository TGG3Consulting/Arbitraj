//! Файл: venues/bsc_dex/pools/market.rs
//! Что делает: подписка на рынок DEX (контракт subscribe_market): монеты и USDT из списка ядра, промежуточные монеты
//! (решение 52: пулы «монета — промежуточная» и «промежуточная — USDT»), пулы из справочника индексатора с проверкой на
//! узле (решение 58: index.rs, обновления — directory/refresh.rs), слушатели пачек пулов (dex.pools_per_sub),
//! наблюдатель новых пулов (события создания), головы сети; необязательная история V4 (history_v4.rs). Возвращает задачи — при новой
//! подписке старые останавливаются (без дублей соединений).
//! Кто вызывает: mod.rs (subscribe_market).
//! От чего зависит: ctx.rs, tokens.rs, node/heads.rs, node/subs.rs, check/cycle.rs, check/pass_cycle.rs (решение 401), node/rpc.rs, discover.rs, listen.rs, created.rs,
//! events.rs, registry.rs, mids.rs, market_mids.rs, history_v4.rs, fee_watch.rs (решение 281), directory/refresh.rs (справочник пулов, решение 58).
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
use super::super::check::{cycle, pass_cycle, quoter_cycle, service_tax, tax_cycle};
use super::super::node::{rpc, subs};
use super::batches::chunks;
use super::created::{self, Watch};
use super::discover::{self, Scanned};
use super::listen::{self, Pools};
use super::{fee_watch, history_v4, index, index_watch, market_mids, mids, multi_pair};
use crate::gateway::urgency::{Urgency as Class, scope};

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
        .filter_map(|a| a.contract.map(|c| (c, a.ticker.clone())))
        .collect();
    let per = subs::per_sub(&p.ctx)?;
    // Без опроса блоков нет свежести — подписка не начинается, ядро повторит её после задания настройки.
    super::super::node::heads::poll_every(&p.ctx)?;
    // Узел отвечает — иначе подписка не начинается (ядро повторит).
    let _: U64 = rpc::call(&p.ctx, "eth_blockNumber", json!([])).await?;
    let mut tasks = vec![tokio::spawn(p.heads.clone().run(p.ctx.clone()))];
    // Фоновый прогрев цены газа и времени блока (решение 89): расчёт читает тёплый кэш, а не тянет узел сам.
    tasks.push(tokio::spawn(super::super::quote::warm::run(
        p.ctx.clone(),
        p.heads.clone(),
        p.gas.clone(),
    )));

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
    // Поиск по фабрикам на своём узле (решение 121, C3) — вторая задача справочника, тот же канал «монета обновлена».
    // С2 (арбитр 28.11.2026): первый проход справочника этой подписки — заново; окончен — «пул готов» (directory/boot.rs).
    super::super::directory::boot::start(&p.ctx, p.signals.ready.clone());
    let (refresh, dir_rx, retry_coins) = index::begin(&p.ctx, &all, usdt, &mid_list).await;
    let mut retry = super::index_retry::Retry::default();
    retry.checked(&p.ctx, None, retry_coins);
    tasks.extend(refresh);
    // Решение 305 (ревью В1): режим справочника — по текущим настройкам и тогда, когда проверка при старте (begin → take)
    // не выполнялась: взятые с ключом не того вида (смена dex.multi_pair) не попадут в найденное. retune идемпотентен.
    let mode = multi_pair::multi_dex(&p.ctx);
    p.ctx.index.with(|s| multi_pair::retune(s, &mode));
    let mut found = discover::find_all(&p.ctx, &all, usdt);
    let bridges=market_mids::wrapped_bridges(&p.ctx,&mid_list);
    let (via, watches) = market_mids::find(&p.ctx, &coins, &mid_list);
    let (bridge_pools, bridge_watches)=market_mids::find(&p.ctx,&bridges.0,&bridges.1);
    let watches=watches.into_iter().chain(bridge_watches).collect::<Vec<_>>();
    let mut seen: HashSet<PoolRef> = found.iter().map(|f| f.reference.clone()).collect();
    found.extend(via.into_iter().chain(bridge_pools).filter(|f| seen.insert(f.reference.clone())));
    let is_mid = |a: &Address| mid_list.iter().any(|m| m.address == *a);
    let known: HashSet<PoolRef> = found.iter().map(|f| f.reference.clone()).collect();
    // Решение 295: запись многомонетного вида, чья ссылка не совпадает с режимом dex.multi_pair (сменили на лету),
    // снимается (multi_pair::entry_fits); решение 305 (ревью В1): запись WooFi — если её пары нет среди найденных
    // (после «да → нет»); у прочих видов — всегда да.
    p.reg.retain(|e| {
        ((e.usdt == usdt && all.contains_key(&e.coin))
            || (is_mid(&e.usdt) && (coins.contains_key(&e.coin) || bridges.0.contains_key(&e.coin) && bridges.1.iter().any(|m|m.address==e.usdt)) && e.coin != e.usdt))
            && multi_pair::entry_fits(&p.ctx, e, &known)
    });
    // Ревью t153 (Н6, решение 339): отметки неудач чтения пулов, которых больше нет среди найденных (и тех, что так
    // и не прочитались — записи в реестре у них нет), — долой.
    p.reg.fails().keep_only(&known);
    p.reg.unread().keep_only(&known);
    // Решение 357 (ревью слияния, Н4): отметки «в обновлении» пулов, которых больше нет среди найденных.
    p.reg.holds().keep_only(|r| known.contains(r));
    // Решение 382 (ревью 3, Н3): учёт доказательств «память неверна» пулов, которых больше нет.
    p.reg.proofs().keep_only(|r| known.contains(r));
    let msg = format!(
        "DEX: пулов из справочника (проверены на узле) с {quote} и промежуточными монетами ({}): {} по {} монетам",
        mid_list.len(),
        found.len(),
        coins.len()
    );
    p.ctx.log(Level::Info, "dex.pool", msg, None).await;

    // Стартовые пачки; налоги монеты проверяются, как только прочитаны её пулы (pools/loading.rs, решение 63).
    let start = chunks(&found, &all, per);
    // Пулы ждут первого чтения — ядро покажет «пулы найдены, загружаются» (решение 61).
    p.loading
        .add(start.iter().flat_map(|(_, c)| c.iter().map(|(f, _)| f)));
    p.loading.publish(p).await;
    for (name, chunk) in start {
        tasks.push(tokio::spawn(listen::run(p.clone(), name, chunk, None)));
    }
    // Проверки (решения 30, 32): налоги (и промежуточных монет — у их шага тоже налог), крючки, сверка с квотером.
    // Проверки монет и крючков — класс Check в очереди узла (решение 61): выше загрузки пулов и цикла квотера.
    let taxes = tax_cycle::taxes(
        p.clone(),
        all.clone(),
        usdt,
        (wake.taxes.clone(), wake.pass.clone()),
    );
    tasks.push(tokio::spawn(scope(Class::Check, taxes)));
    // Налоги служебной монеты USDT — для стоимости его возврата на биржу (решение 67, check/service_tax.rs).
    tasks.push(tokio::spawn(scope(
        Class::Check,
        service_tax::run(p.clone()),
    )));
    let hooks = cycle::hooks(p.clone(), wake.hooks.clone());
    tasks.push(tokio::spawn(scope(Class::Check, hooks)));
    // Решение 401 (ревью t186, Н2): проба пулов менеджера через исполнитель — отдельной задачей, фоном очереди узла (после
    // сделок, проверок и котировок), не задерживая цикл крючков.
    let pass = pass_cycle::run(p.clone(), wake.pass.clone());
    tasks.push(tokio::spawn(scope(Class::Background, pass)));
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
    for family in super::families::all(&p.ctx).into_iter().filter(|f|f.variant.aqua()){
        tasks.push(tokio::spawn(scope(Class::Background, super::aqua_history::run(p.clone(), watches.clone(),family))));
    }
    for w in &watches {
        tasks.push(tokio::spawn(created::run(p.clone(), w.clone())));
    }
    // Комиссия пулов V3-форка от менеджера (решение 281, SquadSwap V3): события смены — перечтение fee() в реестр.
    tasks.push(tokio::spawn(fee_watch::run(p.clone())));
    // Справочник обновил монеты — проверка их пулов и подключение новых (решение 58).
    tasks.push(tokio::spawn(index_watch::watch(
        p.clone(),
        dir_rx,
        retry,
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
