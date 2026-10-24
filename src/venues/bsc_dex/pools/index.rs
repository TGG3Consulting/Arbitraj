//! Файл: venues/bsc_dex/pools/index.rs
//! Что делает: пулы из справочника индексатора → пулы подписки (решение 58). По каждому пулу с USDT или промежуточной
//! монетой (и пулу из события создания): DEX индексатора — по таблице сопоставления (families::classify: не подключена —
//! причина на экране), затем проверка на своём узле (verify.rs) — всех пулов с прочитанных страниц: ликвидность
//! индексатора ни в каком решении не участвует, мелкие пулы отсекает своя мера dex.min_pool_liquidity_usd в подписке.
//! Прошедшие — во «взятые» (их слушает подписка); не прошедшие — причина в справочнике и один раз в журнал; сбой узла —
//! «не проверен» (повтор — index_watch.rs). Пул, исключённый проверкой «кто меняет» (check/emitter.rs), не берётся.
//! Взятые отдаются подписке в порядке загрузки (order.rs, решение 61: по ликвидности индексатора — только порядок).
//! Кто вызывает: pools/market.rs (begin), index_watch.rs, created_log.rs (record).
//! От чего зависит: ctx.rs, directory (состояние, refresh::prepare/restore), families.rs, verify.rs, discover.rs, mids.rs.
//! Настройки: index.network [bsc_dex]; через вызываемые — dex.verify_batch, dex.* [DEX].

use std::collections::{BTreeSet, HashMap, HashSet};

use alloy_primitives::Address;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

use crate::core::types::DirectoryStatus;
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::directory::refresh;
use super::discover::{Found, against};
use super::families;
use super::mids::Mid;
use super::order;
use super::verify::{self, Candidate, Verdict};

/// Итог проверки: взятые пулы (запрошенных монет) и сколько не проверено из-за узла.
pub struct Took {
    pub found: Vec<Found>,
    pub failed: usize,
}

/// Справочник для подписки: встречные токены (USDT и промежуточные), старт и задача обновления
/// (directory/refresh.rs). Возвращает: (задача обновления, приёмник «монета обновлена» для index_watch). Ошибок нет.
pub async fn begin(
    ctx: &Ctx,
    all: &HashMap<Address, String>,
    usdt: Address,
    mids: &[Mid],
) -> (JoinHandle<()>, UnboundedReceiver<Address>) {
    let quotes: HashSet<Address> = std::iter::once(usdt)
        .chain(mids.iter().map(|m| m.address))
        .collect();
    refresh::prepare(ctx, all, quotes.clone());
    let res = match ctx.text("index.network") {
        Ok(net) => refresh::restore(ctx, &net, all).await,
        Err(e) => Err(e),
    };
    if let Err(e) = res {
        let msg = format!("Справочник пулов при старте подписки: {}", e.message);
        ctx.log(Level::Warn, "dex.index", msg, None).await;
    } else if let Err(e) = take(ctx, None).await {
        let msg = format!(
            "Справочник пулов: проверка при старте не выполнена: {}",
            e.message
        );
        ctx.log(Level::Warn, "dex.index", msg, None).await;
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(refresh::run(ctx.clone(), all.clone(), quotes, Some(tx)));
    (task, rx)
}

/// Проверяет пулы справочника (монеты `only` или все) и обновляет итог по каждому.
/// Возвращает: взятые пулы этих монет и число непроверенных из-за узла. Ошибки: NotConfigured (dex.verify_batch).
pub async fn take(ctx: &Ctx, only: Option<&HashSet<Address>>) -> GatewayResult<Took> {
    let mut cands = Vec::new();
    let mut unconnected = BTreeSet::new();
    let mut seen = HashSet::new();
    let mut status = Vec::new();
    for (token, p) in ctx.index.relevant() {
        if only.is_some_and(|o| !o.contains(&token)) || !seen.insert(p.pool.clone()) {
            continue;
        }
        let quote = if p.base == token { p.quote } else { p.base };
        match families::classify(ctx, &p.dex_id) {
            Err(why) => {
                unconnected.insert(p.dex_id.clone());
                status.push((p.pool, DirectoryStatus::NotConnected(why)));
            }
            Ok(family) => cands.push(Candidate {
                family,
                pool: p.pool,
                coin: token,
                quote,
            }),
        }
    }
    let verdicts = verify::verify(ctx, &cands).await?;
    ctx.index.with(|s| {
        for (pool, st) in status {
            s.taken.remove(&pool);
            s.status.insert(pool, st);
        }
    });
    let failed = record(ctx, &cands, verdicts).await;
    if only.is_none() {
        summary(ctx, unconnected).await;
    }
    let found = ctx.index.with(|s| {
        s.taken
            .values()
            .filter(|f| only.is_none_or(|o| o.contains(&f.coin)))
            .cloned()
            .collect()
    });
    Ok(Took {
        found: order::order(ctx, found),
        failed,
    })
}

/// Итоги проверки — в справочник: взят / отклонён (в журнал — один раз на пул и причину) / не проверен (узел).
/// Пул, исключённый проверкой «кто меняет», не берётся. Возвращает: сколько не проверено из-за узла. Ошибок нет.
pub async fn record(ctx: &Ctx, cands: &[Candidate], verdicts: Vec<Verdict>) -> usize {
    let mut failed = 0;
    let mut log = Vec::new();
    ctx.index.with(|s| {
        for (c, v) in cands.iter().zip(verdicts) {
            let st = match v {
                Verdict::Taken(f) => match s.banned.get(&f.reference) {
                    Some(why) => DirectoryStatus::Rejected(why.clone()),
                    None => {
                        s.taken.insert(c.pool.clone(), *f);
                        s.status.insert(c.pool.clone(), DirectoryStatus::Taken);
                        continue;
                    }
                },
                Verdict::Rejected(why) => {
                    if s.logged.insert(format!("{} {why}", c.pool)) {
                        log.push(format!("{} {} ({}): {why}", c.family.id, c.pool, c.coin));
                    }
                    DirectoryStatus::Rejected(why)
                }
                Verdict::Failed(_) => {
                    failed += 1;
                    DirectoryStatus::Unchecked
                }
            };
            s.taken.remove(&c.pool);
            s.status.insert(c.pool.clone(), st);
        }
    });
    for r in log {
        ctx.log(
            Level::Warn,
            "dex.index",
            format!("Пул из справочника отклонён: {r}"),
            None,
        )
        .await;
    }
    failed
}

/// Одна запись журнала на смену списка DEX с пулами монет, но не подключённых. Ошибок нет.
async fn summary(ctx: &Ctx, now: BTreeSet<String>) {
    let changed = ctx.index.with(|s| {
        let c = s.unconnected != now;
        s.unconnected = now.clone();
        c
    });
    if changed && !now.is_empty() {
        let list: Vec<String> = now.into_iter().collect();
        let msg = format!(
            "Справочник: DEX с пулами монет, но не подключены — {}",
            list.join(", ")
        );
        ctx.log(Level::Info, "dex.index", msg, None).await;
    }
}

/// Взятые пулы со встречным токеном (монета — другая валюта пула и есть в списке) — в порядке загрузки
/// (pools/order.rs). Ошибок нет.
pub fn taken_against(ctx: &Ctx, coins: &HashMap<Address, String>, quote: Address) -> Vec<Found> {
    let all: Vec<Found> = ctx.index.with(|s| s.taken.values().cloned().collect());
    let mine = all
        .iter()
        .filter_map(|f| against(f, quote))
        .filter(|f| f.coin != quote && coins.contains_key(&f.coin))
        .collect();
    order::order(ctx, mine)
}
