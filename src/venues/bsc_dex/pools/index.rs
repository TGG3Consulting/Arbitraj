//! Файл: venues/bsc_dex/pools/index.rs
//! Что делает: пулы из справочника индексатора → пулы подписки (решение 58). По каждому пулу с USDT или промежуточной
//! монетой: DEX индексатора — по таблице сопоставления (families::classify: не подключена — причина на экране),
//! ликвидность по индексатору не ниже index.min_reserve_usd, затем проверка на своём узле (verify.rs). Прошедшие —
//! в «взятые» (их слушает подписка); не прошедшие — причина в справочнике и один раз в журнал. При старте подписки —
//! из сохранённого справочника; дальше — по каждой обновлённой монете (directory/refresh.rs → канал) с подключением
//! новых пулов к идущей подписке (attach.rs) и обновлением своей комиссии пар V2 в реестре.
//! Кто вызывает: pools/market.rs (begin, watch).
//! От чего зависит: ctx.rs, directory (состояние, refresh::prepare/restore), families.rs, verify.rs, attach.rs,
//! created.rs (Watch), discover.rs (against), check/recheck.rs.
//! Настройки: index.min_reserve_usd, index.network [bsc_dex]; через вызываемые — dex.* [DEX].

use std::collections::{BTreeSet, HashMap, HashSet};

use alloy_primitives::Address;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

use crate::core::types::DirectoryStatus;
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::check::recheck::Recheck;
use super::super::ctx::Ctx;
use super::super::directory::refresh;
use super::attach::attach;
use super::created::Watch;
use super::discover::{Found, against};
use super::families;
use super::listen::Pools;
use super::mids::Mid;
use super::verify::{self, Candidate};

/// Справочник для подписки: встречные токены (USDT и промежуточные), старт (start) и задача обновления
/// (directory/refresh.rs). Возвращает: (задача обновления, приёмник «монета обновлена» для watch). Ошибок нет.
pub async fn begin(
    ctx: &Ctx,
    all: &HashMap<Address, String>,
    usdt: Address,
    mids: &[Mid],
) -> (JoinHandle<()>, UnboundedReceiver<Address>) {
    let quotes: HashSet<Address> = std::iter::once(usdt)
        .chain(mids.iter().map(|m| m.address))
        .collect();
    start(ctx, all, quotes.clone()).await;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(refresh::run(ctx.clone(), all.clone(), quotes, Some(tx)));
    (task, rx)
}

/// Старт подписки: монеты и встречные токены — в справочник, сохранённый справочник — из SQLite, его пулы —
/// проверка на узле. Ошибок нет: не задано или сбой — в журнал, пулов из справочника нет до обновления.
pub async fn start(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: HashSet<Address>) {
    refresh::prepare(ctx, coins, quotes);
    let res = match ctx.text("index.network") {
        Ok(net) => refresh::restore(ctx, &net, coins).await,
        Err(e) => Err(e),
    };
    let res = match res {
        Ok(()) => take(ctx, None).await.map(|_| ()),
        Err(e) => Err(e),
    };
    if let Err(e) = res {
        let msg = format!("Справочник пулов при старте подписки: {}", e.message);
        ctx.log(Level::Warn, "dex.index", msg, None).await;
    }
}

/// Проверяет пулы справочника (монеты `only` или все) и обновляет итог по каждому. Возвращает: взятые пулы этих монет.
/// Ошибки: NotConfigured (index.min_reserve_usd), узел (итог «не проверен» остаётся).
pub async fn take(ctx: &Ctx, only: Option<&HashSet<Address>>) -> GatewayResult<Vec<Found>> {
    let min = ctx.dec("index.min_reserve_usd")?;
    let list = ctx.index.relevant();
    let mut cands = Vec::new();
    let mut status: Vec<(String, DirectoryStatus)> = Vec::new();
    let mut unconnected = BTreeSet::new();
    let mut seen = HashSet::new();
    for (token, p) in list {
        if only.is_some_and(|o| !o.contains(&token)) || !seen.insert(p.pool.clone()) {
            continue;
        }
        let quote = if p.base == token { p.quote } else { p.base };
        match families::classify(ctx, &p.dex_id) {
            Err(why) => {
                unconnected.insert(p.dex_id.clone());
                status.push((p.pool, DirectoryStatus::NotConnected(why)));
            }
            Ok(_) if p.reserve_usd.is_none_or(|r| r < min) => {
                status.push((p.pool, DirectoryStatus::Small));
            }
            Ok(family) => cands.push(Candidate {
                family,
                pool: p.pool,
                coin: token,
                quote,
            }),
        }
    }
    let results = verify::verify(ctx, &cands).await?;
    let mut rejected = Vec::new();
    let taken = ctx.index.with(|s| {
        for (pool, st) in status {
            s.taken.remove(&pool);
            s.status.insert(pool, st);
        }
        for (c, r) in cands.iter().zip(results) {
            match r {
                Ok(f) => {
                    s.status.insert(c.pool.clone(), DirectoryStatus::Taken);
                    s.taken.insert(c.pool.clone(), f);
                }
                Err(why) => {
                    s.taken.remove(&c.pool);
                    if s.logged.insert(format!("{} {why}", c.pool)) {
                        rejected.push(format!("{} {} ({}): {why}", c.family.id, c.pool, c.coin));
                    }
                    s.status
                        .insert(c.pool.clone(), DirectoryStatus::Rejected(why));
                }
            }
        }
        let changed = only.is_none() && s.unconnected != unconnected;
        if changed {
            s.unconnected = unconnected.clone();
        }
        let taken: Vec<Found> = s
            .taken
            .values()
            .filter(|f| only.is_none_or(|o| o.contains(&f.coin)))
            .cloned()
            .collect();
        (taken, changed)
    });
    for r in rejected {
        ctx.log(
            Level::Warn,
            "dex.index",
            format!("Пул из справочника отклонён: {r}"),
            None,
        )
        .await;
    }
    if taken.1 && !unconnected.is_empty() {
        let list: Vec<String> = unconnected.into_iter().collect();
        let msg = format!(
            "Справочник: DEX с пулами монет, но не подключены — {}",
            list.join(", ")
        );
        ctx.log(Level::Info, "dex.index", msg, None).await;
    }
    Ok(taken.0)
}

/// Взятые пулы со встречным токеном (монета — другая валюта пула и есть в списке). Ошибок нет.
pub fn taken_against(ctx: &Ctx, coins: &HashMap<Address, String>, quote: Address) -> Vec<Found> {
    let all: Vec<Found> = ctx.index.with(|s| s.taken.values().cloned().collect());
    all.iter()
        .filter_map(|f| against(f, quote))
        .filter(|f| f.coin != quote && coins.contains_key(&f.coin))
        .collect()
}

/// Работает, пока жива подписка: по обновлённым монетам справочника — проверка их пулов, новые — к подписке
/// каждого наблюдателя (USDT и промежуточные) по его встречному токену; своя комиссия пар V2 — в реестр.
/// После подключения новых — «новые пулы» циклам проверок. Ошибок не возвращает: всё — в журнал.
pub async fn watch(
    p: Pools,
    mut rx: UnboundedReceiver<Address>,
    watches: Vec<Watch>,
    wake: Recheck,
) {
    while let Some(first) = rx.recv().await {
        let mut tokens = HashSet::from([first]);
        while let Ok(t) = rx.try_recv() {
            tokens.insert(t);
        }
        let found = match take(&p.ctx, Some(&tokens)).await {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("Справочник: проверка пулов не выполнена: {}", e.message);
                p.ctx.log(Level::Warn, "dex.index", msg, None).await;
                continue;
            }
        };
        for f in &found {
            if f.fee_pips.is_some() && f.family.proto == families::Proto::V2 {
                p.reg.update(&f.reference, |e| e.pool.fee_pips = f.fee_pips);
            }
        }
        let mut n = 0;
        for w in &watches {
            let mine: Vec<Found> = found.iter().filter_map(|x| against(x, w.usdt)).collect();
            match attach(&p, w, mine, true).await {
                Ok(k) => n += k,
                Err(e) => p.ctx.log(Level::Error, "dex.index", e.message, None).await,
            }
        }
        if n > 0 {
            wake.new_pools();
        }
    }
}
