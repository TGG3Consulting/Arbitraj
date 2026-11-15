//! Файл: venues/bsc_dex/pools/listen_site.rs
//! Что делает: решение 295 — лог контракта многомонетного пула, у которого в пачке пары `PoolRef::Multi`: одна
//! перечитка на контракт для всех глубоких пар (событие у этих видов — только «перечитать», events_curve.rs,
//! events_curve_ng.rs, events_balancer.rs, events_stable_u.rs), мелкие пары — пробуждение по событию ликвидности (как
//! мелкий пул со своим контрактом). Правила лога — как у listen_log.rs: уже учтённый лог пропускается (synced_block
//! первой глубокой пары — все пары пишутся вместе), реорг (removed) — перечитка на текущем блоке, иначе — на блоке лога;
//! сбой чтения — все пары контракта недостоверны и событие Invalidated по их монетам, конец сессии с причиной.
//! Кто вызывает: pools/session.rs (on_log: лог не нашёл пул по Singleton / Contract).
//! От чего зависит: admit.rs (wake, Members), events.rs, listen_log.rs (latest), reload_many.rs (load_group),
//! registry.rs, invalidated.rs.
//! Настройки: через вызываемые — dex.load_batch и настройки чтения вида.

use alloy::rpc::types::Log;

use crate::core::types::PoolRef;
use crate::storage::journal::Level;

use super::admit::{self, Members};
use super::events::{self, Applied};
use super::group::Group;
use super::listen::{End, Pools};
use super::{listen_log, reload_many};

/// Что делает: лог контракта пар Multi. Принимает: окружение, пачку, её пулы, лог. Возвращает: false — пар Multi с
/// этим адресом в пачке нет (лог — не им); true — обработан. Ошибки: End — сбой чтения или ядро закрыло канал.
pub async fn on_log(p: &Pools, g: &mut Group, m: &mut Members, log: &Log) -> Result<bool, End> {
    let addr = log.address();
    let mine: Vec<usize> = m
        .pools
        .iter()
        .enumerate()
        .filter(|(_, (f, _))| matches!(f.reference, PoolRef::Multi { contract, .. } if contract == addr))
        .map(|(i, _)| i)
        .collect();
    let Some(&first) = mine.first() else {
        return Ok(false);
    };
    let (Some(block), Some(index)) = (log.block_number, log.log_index) else {
        return Err(End::Broken(
            "лог подписки без blockNumber/logIndex — порядок событий неизвестен".into(),
        ));
    };
    let family = m.pools[first].0.family;
    let topic = log.topic0().copied();
    let deep: Vec<usize> = mine
        .iter()
        .copied()
        .filter(|&i| m.deep.get(i).copied().unwrap_or(true))
        .collect();
    // Мелкие пары — пробуждение по событию ликвидности (мелкий пул со своим контрактом слушается только им).
    if !log.removed && topic.is_some_and(|t| events::liquidity_topics(family).contains(&t)) {
        for &i in mine.iter().filter(|i| !deep.contains(i)) {
            admit::wake(p, g, m, i, block).await?;
        }
    }
    if deep.is_empty() || !topic.is_some_and(|t| events::topics(family).contains(&t)) {
        return Ok(true);
    }
    let r0 = m.pools[deep[0]].0.reference.clone();
    let Some(entry) = p.reg.get(&r0) else {
        return Ok(true);
    };
    if !log.removed && entry.has_applied(block, index) {
        return Ok(true);
    }
    let applied = if log.removed {
        Applied::Refresh("реорг: событие отменено (removed)".into())
    } else {
        p.reg
            .update(&r0, |e| events::apply(e, log))
            .unwrap_or(Applied::Ignored)
    };
    let why = match applied {
        Applied::Ignored => return Ok(true),
        Applied::Refresh(w) => w,
        // У многомонетных видов точечного применения нет (только перечитка) — на всякий случай тоже перечитка.
        Applied::Updated | Applied::Bins(_) => "событие пула".into(),
    };
    let msg = format!(
        "{} {addr:#x}: {why} — перечитываю пары пула одним чтением ({})",
        family.title,
        deep.len()
    );
    p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
    let at = if log.removed {
        listen_log::latest(p).await.map_err(|e| End::Broken(e.message))?
    } else {
        block
    };
    let items: Vec<(&super::discover::Found, &str)> = deep
        .iter()
        .map(|&i| (&m.pools[i].0, m.pools[i].1.as_str()))
        .collect();
    let res = reload_many::load_group(p, &items, at).await?;
    if let Some(e) = res.into_iter().find_map(Result::err) {
        for (f, t) in &items {
            p.reg.update(&f.reference, |x| x.valid = false);
            super::invalidated::send(p, f.coin, t.to_string());
        }
        return Err(End::Broken(e.message));
    }
    p.signals.pool_ready();
    Ok(true)
}
