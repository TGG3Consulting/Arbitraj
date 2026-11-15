//! Файл: venues/bsc_dex/pools/reload_many.rs
//! Что делает: чтение многих пулов пачками (решение 61) — приём пачки и повтор чтения: группы пулов, у каждой свежий
//! блок, внутри — ступени load_batch.rs; прочитанные пулы — в реестр и ядру (как reload::load_one). Решение 298:
//! пулы видов, читаемых по одному последовательно, — каждый на своём свежем блоке (fresh_each).
//! Кто вызывает: pools/admit.rs, pools/reload.rs (retry); load_group — ещё per_block.rs, listen_site.rs (решение 295).
//! От чего зависит: load_batch.rs, load.rs, listen_log.rs, http_wait.rs, registry.rs.
//! Настройки: dex.load_batch [bsc_dex]; через load_batch.rs — dex.multicall3, dex.tick_words, dex.tick_lens, dex.state_view.

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayError;

use super::discover::Found;
use super::listen::{End, Pools};
use super::load_batch::{self, Want};
use super::registry::Registry;
use super::{http_wait, listen_log, load};

/// Чтение многих пулов пачками (load_batch.rs) группами: у каждой группы — свежий блок (последний известный, не
/// раньше `min_block`), чтобы узел ещё отдавал его состояние, пока очередь дойдёт до следующих групп; события пула до
/// этого блока включительно уже учтены в чтении и при применении пропускаются (registry.rs: synced_block).
/// В группе не больше dex.load_batch / 2 пулов: у пула в ступени 1 до двух вызовов — ступень 1 группы помещается
/// в одну пачку Multicall3. Решение 298: пулы видов, читаемых по одному последовательными вызовами (fresh_each), —
/// каждый на СВОЁМ свежем блоке после остальных пулов группы: иначе к концу группы чтение шло на блоке, которого у
/// не архивного узла уже нет; остальные пулы группы — как раньше, вместе на одном блоке. Прочитанные — ядру.
/// Принимает: (пул, тикер) по порядку, наименьший блок. Возвращает: по каждому (в том же порядке) Ok — прочитан,
/// Err(причина) — нет. Ошибки: End — ядро закрыло канал.
pub async fn load_many(
    p: &Pools,
    items: &[(&Found, &str)],
    min_block: u64,
) -> Result<Vec<Result<(), GatewayError>>, End> {
    let per = match load_batch::batch(&p.ctx) {
        Ok(n) => (n / 2).max(1),
        Err(e) => return Ok(items.iter().map(|_| Err(e.clone())).collect()),
    };
    let mut out: Vec<Option<Result<(), GatewayError>>> = items.iter().map(|_| None).collect();
    for (n, group) in items.chunks(per).enumerate() {
        let base = n * per;
        let (each, together): (Vec<usize>, Vec<usize>) =
            (0..group.len()).partition(|&k| fresh_each(group[k].0));
        if !together.is_empty() {
            let list: Vec<(&Found, &str)> = together.iter().map(|&k| group[k]).collect();
            for (&k, r) in together.iter().zip(at_fresh(p, &list, min_block).await?) {
                out[base + k] = Some(r);
            }
        }
        for k in each {
            let r = at_fresh(p, &group[k..=k], min_block).await?;
            out[base + k] = r.into_iter().next();
        }
    }
    Ok(out
        .into_iter()
        .map(|o| {
            o.unwrap_or_else(|| {
                Err(load_batch::miss(
                    "состояние пула не собрано (внутренняя ошибка)",
                ))
            })
        })
        .collect())
}

/// Что делает: решение 298 — пулу свой свежий блок: вид читается по одному последовательными вызовами
/// (load_batch::one_by_one), и это не пара многомонетного пула (PoolRef::Multi: пары контракта читаются одним чтением
/// на одном блоке, решение 295 — остаются в группе). Принимает: найденный пул. Возвращает: true — свой блок. Ошибок нет.
pub fn fresh_each(f: &Found) -> bool {
    load_batch::one_by_one(f.family) && !matches!(f.reference, PoolRef::Multi { .. })
}

/// Что делает: пулы на свежем блоке (последний известный, не раньше `min_block` и synced_block их записей) — ждёт
/// узел HTTP до него и читает
/// одной группой (load_group). Принимает: (пул, тикер), наименьший блок. Возвращает: по каждому Ok или причину (узел
/// HTTP не дошёл — та же причина у всех). Ошибки: End — ядро закрыло канал.
async fn at_fresh(
    p: &Pools,
    items: &[(&Found, &str)],
    min_block: u64,
) -> Result<Vec<Result<(), GatewayError>>, End> {
    // Ревью 298: не раньше учтённой позиции записей (synced_block) — события WS, уже применённые к записи (в т.ч. к
    // недостоверной, ждущей повтора, и отмеченные stale_read::defer), могут быть новее головы узла HTTP; чтение ниже
    // откатило бы запись (load.rs renewed) и потеряло их. Узел HTTP до такого блока — ждём (http_reached).
    let floor = floor_block(&p.reg, items, min_block);
    let block = p.heads.latest().map_or(floor, |h| h.number.max(floor));
    match http_wait::http_reached(p, block).await {
        Ok(()) => load_group(p, items, block).await,
        Err(e) => Ok(items.iter().map(|_| Err(e.clone())).collect()),
    }
}

/// Что делает: ревью 298 — наименьший блок чтения пулов: `min_block` и synced_block их записей в реестре (чтение ниже
/// откатило бы запись и потеряло применённые события). Принимает: реестр, (пул, тикер), наименьший блок.
/// Возвращает: наибольшее из них (записи нет — только `min_block`). Ошибок нет.
pub fn floor_block(reg: &Registry, items: &[(&Found, &str)], min_block: u64) -> u64 {
    items
        .iter()
        .filter_map(|(f, _)| reg.get(&f.reference).map(|e| e.synced_block))
        .fold(min_block, u64::max)
}

/// Одна группа на одном блоке: пачечное чтение и отправка ядру. Решение 295: пары одного многомонетного пула в группе
/// читаются одним чтением (load_batch.rs → load_site.rs) — так их перечитывают per_block.rs и listen_site.rs.
/// Принимает: (пул, тикер), блок. Возвращает: по каждому Ok или причину. Ошибки: End — ядро закрыло канал.
pub async fn load_group(
    p: &Pools,
    items: &[(&Found, &str)],
    block: u64,
) -> Result<Vec<Result<(), GatewayError>>, End> {
    let known: Vec<_> = items.iter().map(|(f, _)| p.reg.get(&f.reference)).collect();
    let wants: Vec<Want> = items
        .iter()
        .zip(&known)
        .map(|((f, _), e)| match e {
            Some(e) => Want {
                family: e.family,
                reference: &f.reference,
                spacing: e.pool.tick_spacing,
                step: e.pool.bin_step,
                usdt: e.usdt,
                token0: e.pool.token0,
                token1: e.pool.token1,
            },
            None => Want {
                family: f.family,
                reference: &f.reference,
                spacing: f.tick_spacing,
                step: f.bin_step,
                usdt: listen_log::found_usdt(f),
                token0: f.token0,
                token1: f.token1,
            },
        })
        .collect();
    let loaded = load_batch::states(&p.ctx, &p.tokens, &wants, block).await;
    let mut out = Vec::with_capacity(items.len());
    for (((f, ticker), e), l) in items.iter().zip(&known).zip(loaded) {
        let l = match l {
            Ok(l) => l,
            Err(why) => {
                out.push(Err(why));
                continue;
            }
        };
        let entry = match e {
            Some(e) => load::renewed(e, block, l),
            None => load::built(&p.ctx, f, ticker, listen_log::found_usdt(f), block, l),
        };
        listen_log::note_refill(p, &entry);
        let pool = entry.pool.clone();
        p.reg.put(entry);
        p.loading.done(&f.reference);
        listen_log::emit(p, pool)?;
        out.push(Ok(()));
    }
    // Прочитанные уходят из «загружается» (loading.rs).
    p.loading.publish(p).await;
    Ok(out)
}
