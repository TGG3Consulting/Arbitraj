//! Файл: venues/bsc_dex/pools/index_watch.rs
//! Что делает: справочник пулов во время подписки (решение 58). Перепроверка и подключение новых пулов к идущей
//! подписке (attach.rs): по обновлённым монетам (directory/refresh.rs), целиком — при изменении любой настройки dex.*
//! (вид DEX, адреса, включатель — без перезапуска) и при повторе через dex.verify_retry_s, если узел не ответил на
//! часть проверки. Своя комиссия пар V2 — в реестр (в том числе её снятие при смене источника на «одна на DEX»);
//! её же перечитывает сверка с квотером (refresh_fees).
//! Кто вызывает: pools/market.rs (watch), check/quoter_cycle.rs (refresh_fees).
//! От чего зависит: ctx.rs, index.rs, verify.rs, attach.rs, created.rs (Watch), discover.rs, families.rs,
//! directory (pool_key), settings/live.rs, check/recheck.rs.
//! Настройки: dex.verify_retry_s [bsc_dex]; изменения dex.*.

use std::collections::HashSet;
use std::time::Duration;

use alloy_primitives::Address;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::settings::live::Watch as Changes;
use crate::storage::journal::Level;

use super::super::check::recheck::Recheck;
use super::super::directory::pool_key;
use super::attach::attach;
use super::created::Watch;
use super::discover::{Found, against};
use super::families::Proto;
use super::index;
use super::listen::Pools;
use super::verify::{self, Candidate, Verdict};

/// Работает, пока жива подписка (см. шапку). Ошибок не возвращает: всё — в журнал.
pub async fn watch(
    p: Pools,
    mut rx: UnboundedReceiver<Address>,
    watches: Vec<Watch>,
    wake: Recheck,
) {
    let mut changes = Changes::new(p.ctx.settings.subscribe(), vec!["dex.".into()]);
    let mut retry: Option<Duration> = None;
    loop {
        let pause = async {
            match retry {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending().await,
            }
        };
        // None — все монеты (настройки изменились или повтор после сбоя узла).
        let only: Option<HashSet<Address>> = tokio::select! {
            t = rx.recv() => match t {
                None => return,
                Some(first) => {
                    let mut set = HashSet::from([first]);
                    while let Ok(x) = rx.try_recv() {
                        set.insert(x);
                    }
                    Some(set)
                }
            },
            c = changes.next() => if c.is_none() { return } else { None },
            _ = pause => None,
        };
        retry = None;
        let took = match index::take(&p.ctx, only.as_ref()).await {
            Ok(t) => t,
            Err(e) => {
                let msg = format!("Справочник: проверка пулов не выполнена: {}", e.message);
                p.ctx.log(Level::Warn, "dex.index", msg, None).await;
                continue;
            }
        };
        if took.failed > 0 {
            retry = p
                .ctx
                .int("dex.verify_retry_s")
                .ok()
                .map(Duration::from_secs);
        }
        fees(&p, &took.found);
        let mut n = 0;
        for w in &watches {
            let mine: Vec<Found> = took
                .found
                .iter()
                .filter_map(|x| against(x, w.usdt))
                .collect();
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

/// Комиссия пар V2 из проверки — в реестр: своя у пары или None (одна на DEX, dex.v2_fee_pct). Ошибок нет.
fn fees(p: &Pools, found: &[Found]) {
    for f in found.iter().filter(|f| f.family.proto == Proto::V2) {
        p.reg.update(&f.reference, |e| e.pool.fee_pips = f.fee_pips);
    }
}

/// Перечитывает комиссию пар V2 в реестре той же проверкой на узле (сверка с квотером, check/quoter_cycle.rs):
/// своя у пары — заново с пары, источник «одна на DEX» — прежняя своя снимается. Ошибок нет: сбой — в журнал.
pub async fn refresh_fees(p: &Pools) {
    let mut cands = Vec::new();
    for r in p.reg.refs() {
        let Some(e) = p.reg.get(&r) else { continue };
        if e.family.proto != Proto::V2 {
            continue;
        }
        if verify::fee_method(&p.ctx, e.family)
            .ok()
            .flatten()
            .is_none()
        {
            if e.pool.fee_pips.is_some() {
                p.reg.update(&r, |x| x.pool.fee_pips = None);
            }
            continue;
        }
        cands.push(Candidate {
            family: e.family,
            pool: pool_key(&r),
            coin: e.coin,
            quote: e.usdt,
        });
    }
    if cands.is_empty() {
        return;
    }
    match verify::verify(&p.ctx, &cands).await {
        Ok(list) => {
            let found: Vec<Found> = list
                .into_iter()
                .filter_map(|v| match v {
                    Verdict::Taken(f) => Some(*f),
                    _ => None,
                })
                .collect();
            fees(p, &found);
        }
        Err(e) => {
            let msg = format!("Комиссия пар V2 не перечитана: {}", e.message);
            p.ctx.log(Level::Warn, "dex.index", msg, None).await;
        }
    }
}
