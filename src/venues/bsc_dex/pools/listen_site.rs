//! Файл: venues/bsc_dex/pools/listen_site.rs
//! Что делает: решение 295 — лог контракта многомонетного пула, у которого в пачке пары `PoolRef::Multi`: одна
//! перечитка на контракт для глубоких пар, у которых лог ещё не учтён (событие у этих видов — только «перечитать»,
//! events_curve.rs, events_curve_ng.rs, events_balancer.rs, events_stable_u.rs); мелкие пары — пробуждение по событию
//! ликвидности (как мелкий пул со своим контрактом), не больше одного раза на лог. Правила лога — как у listen_log.rs,
//! но по КАЖДОЙ паре (ревью 295, 1а): лог пропускается, только если он учтён у всех глубоких пар контракта в пачке
//! (отметки пар могут разойтись — reload::load_one при пробуждении / повторе читает одну пару); реорг (removed) — все
//! глубокие пары на текущем блоке, иначе — неучтённые на блоке лога (stale). Пары одного адреса в двух пачках
//! подписки: каждая пачка получает лог своей подпиской и перечитывает свои пары. Сбой чтения — перечитываемые пары
//! недостоверны и событие Invalidated по их монетам, конец сессии с причиной; решение 298 — если у всех упавших пар
//! StaleData (у узла нет состояния блока), сессия не рвётся: упавшие пары — в очередь повтора (stale_read.rs), а пары,
//! уже ждущие повтора, по логу не читаются (позиция лога отмечается).
//! Кто вызывает: pools/session.rs (on_log: лог не нашёл пул по Singleton / Contract).
//! От чего зависит: admit.rs (wake, Members), events.rs, listen_log.rs (latest), reload_many.rs (load_group),
//! registry.rs, invalidated.rs, stale_read.rs (решение 298), refresh_urgency.rs (решение 314).
//! Настройки: через вызываемые — dex.load_batch и настройки чтения вида; dex.event_refresh_urgency (класс очереди узла).

use alloy::rpc::types::Log;

use crate::core::types::PoolRef;
use crate::storage::journal::Level;

use super::admit::{self, Members};
use super::events::{self, Applied};
use super::group::Group;
use super::listen::{End, Pools};
use super::registry::Entry;
use super::{listen_log, refresh_urgency, reload_many, stale_read};

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
    // Мелкие пары — пробуждение по событию ликвидности (мелкий пул со своим контрактом слушается только им). Лог
    // одного контракта может прийти дважды (глубокая пара в полной подписке, мелкая — в подписке ликвидности,
    // group.rs): пара, уже прочитанная на этом логе или позже (has_applied — пробуждение читает на блоке лога), второй
    // раз не будится — один лог, одно действие на пару.
    if !log.removed && topic.is_some_and(|t| events::liquidity_topics(family).contains(&t)) {
        for i in mine.iter().copied().filter(|i| !deep.contains(i)) {
            let r = &m.pools[i].0.reference;
            if p.reg.get(r).is_some_and(|e| e.has_applied(block, index)) {
                continue;
            }
            admit::wake(p, g, m, i, block).await?;
        }
    }
    if deep.is_empty() || !topic.is_some_and(|t| events::topics(family).contains(&t)) {
        return Ok(true);
    }
    // Ревью 295 (1а): лог пропускается, только если он учтён у ВСЕХ глубоких пар контракта в пачке; перечитываются
    // пары, у которых он не учтён (у каждой своя отметка synced_block / synced_log — load_one мог поднять одну пару).
    let entries: Vec<(usize, Entry)> = deep
        .iter()
        .filter_map(|&i| p.reg.get(&m.pools[i].0.reference).map(|e| (i, e)))
        .collect();
    let list: Vec<Entry> = entries.iter().map(|(_, e)| e.clone()).collect();
    let pick = stale(&list, block, index, log.removed);
    let Some(&k0) = pick.first() else {
        return Ok(true);
    };
    let r0 = entries[k0].1.pool.reference.clone();
    let applied = if log.removed {
        Applied::Refresh("реорг: событие отменено (removed)".into())
    } else {
        p.reg
            .update(&r0, |e| events::apply(e, log))
            .unwrap_or(Applied::Ignored)
    };
    let why = match applied {
        Applied::Ignored => return Ok(true),
        // Решение 325: у Fee (только Algebra Integral, не многомонетный вид) — та же перечитка.
        Applied::Refresh(w) | Applied::Fee(w) => w,
        // У многомонетных видов точечного применения нет (только перечитка) — на всякий случай тоже перечитка.
        Applied::Updated | Applied::Bins(_) => "событие пула".into(),
    };
    // Решение 298: пары в очереди повтора не читаются (узел на старом блоке не спрашиваем, журнал не пишем) — позиция
    // лога отмечается, повтор прочитает их не раньше неё (stale_read::defer).
    let (wait, deep): (Vec<usize>, Vec<usize>) = pick
        .iter()
        .map(|&k| entries[k].0)
        .partition(|&i| stale_read::waiting(&m.missing, i));
    if !log.removed {
        for &i in &wait {
            stale_read::defer(&p.reg, &m.pools[i].0.reference, block, index);
        }
    }
    if deep.is_empty() {
        return Ok(true);
    }
    let msg = format!(
        "{} {addr:#x}: {why} — перечитываю пары пула одним чтением ({})",
        family.title,
        deep.len()
    );
    p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
    // Решение 314: чтения по событию — классом dex.event_refresh_urgency (refresh_urgency.rs; не задана — как раньше).
    let at = if log.removed {
        refresh_urgency::urgent(&p.ctx, listen_log::latest(p))
            .await
            .map_err(|e| End::Broken(e.message))?
    } else {
        block
    };
    let items: Vec<(&super::discover::Found, &str)> = deep
        .iter()
        .map(|&i| (&m.pools[i].0, m.pools[i].1.as_str()))
        .collect();
    let res = refresh_urgency::urgent(&p.ctx, reload_many::load_group(p, &items, at)).await?;
    // Прочая ошибка (не StaleData) хоть у одной пары — как раньше: все перечитываемые пары недостоверны, конец сессии.
    if let Some(e) = res
        .iter()
        .find_map(|r| r.as_ref().err().filter(|e| !stale_read::is_stale(e)))
    {
        for (f, t) in &items {
            p.reg.update(&f.reference, |x| x.valid = false);
            super::invalidated::send(p, f.coin, t.to_string());
        }
        return Err(End::Broken(e.message.clone()));
    }
    // Решение 298: у узла нет состояния блока — упавшие пары недостоверны и в очередь повтора, сессия идёт дальше.
    for (k, r) in res.into_iter().enumerate() {
        let Err(e) = r else { continue };
        let (f, t) = items[k];
        p.reg.update(&f.reference, |x| x.valid = false);
        super::invalidated::send(p, f.coin, t.to_string());
        if let Some(entry) = p.reg.get(&f.reference) {
            stale_read::note(p, &entry, at, &e).await;
        }
        stale_read::requeue(&mut m.missing, &mut m.abandoned, deep[k]);
    }
    p.signals.pool_ready();
    Ok(true)
}

/// Что делает: какие глубокие пары контракта перечитать на лог (ревью 295, 1а): реорг — все; иначе — пары, у которых
/// лог ещё не учтён (Entry::has_applied). Пусто — лог учтён у всех, пропуск. Принимает: записи пар, блок и номер лога,
/// отменён ли лог. Возвращает: индексы записей. Ошибок нет.
pub fn stale(entries: &[Entry], block: u64, index: u64, removed: bool) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| removed || !e.has_applied(block, index))
        .map(|(k, _)| k)
        .collect()
}
