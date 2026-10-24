//! Файл: venues/bsc_dex/pools/reload_many.rs
//! Что делает: чтение многих пулов пачками (решение 61) — приём пачки и повтор чтения: группы пулов, у каждой свежий
//! блок, внутри — ступени load_batch.rs; прочитанные пулы — в реестр и ядру (как reload::load_one).
//! Кто вызывает: pools/admit.rs, pools/reload.rs (retry).
//! От чего зависит: load_batch.rs, load.rs, listen_log.rs, http_wait.rs, registry.rs.
//! Настройки: dex.load_batch [bsc_dex]; через load_batch.rs — dex.multicall3, dex.tick_words, dex.tick_lens, dex.state_view.

use super::discover::Found;
use super::listen::{End, Pools};
use super::load_batch::{self, Want};
use super::{http_wait, listen_log, load};

/// Чтение многих пулов пачками (load_batch.rs) группами: у каждой группы — свежий блок (последний известный, не
/// раньше `min_block`), чтобы узел ещё отдавал его состояние, пока очередь дойдёт до следующих групп; события пула до
/// этого блока включительно уже учтены в чтении и при применении пропускаются (registry.rs: synced_block).
/// В группе не больше dex.load_batch / 2 пулов: у пула в ступени 1 до двух вызовов — ступень 1 группы помещается
/// в одну пачку Multicall3. Прочитанные — ядру.
/// Принимает: (пул, тикер) по порядку, наименьший блок. Возвращает: по каждому Ok — прочитан, Err(причина) — нет.
/// Ошибки: End — ядро закрыло канал.
pub async fn load_many(
    p: &Pools,
    items: &[(&Found, &str)],
    min_block: u64,
) -> Result<Vec<Result<(), String>>, End> {
    let per = match load_batch::batch(&p.ctx) {
        Ok(n) => (n / 2).max(1),
        Err(e) => return Ok(items.iter().map(|_| Err(e.message.clone())).collect()),
    };
    let mut out = Vec::with_capacity(items.len());
    for group in items.chunks(per) {
        let block = p
            .heads
            .latest()
            .map_or(min_block, |h| h.number.max(min_block));
        match http_wait::http_reached(p, block).await {
            Ok(()) => out.extend(load_group(p, group, block).await?),
            Err(e) => out.extend(group.iter().map(|_| Err(e.message.clone()))),
        }
    }
    Ok(out)
}

/// Одна группа на одном блоке: пачечное чтение и отправка ядру. Ошибки: End — ядро закрыло канал.
async fn load_group(
    p: &Pools,
    items: &[(&Found, &str)],
    block: u64,
) -> Result<Vec<Result<(), String>>, End> {
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
                token1: e.pool.token1,
            },
            None => Want {
                family: f.family,
                reference: &f.reference,
                spacing: f.tick_spacing,
                step: f.bin_step,
                usdt: listen_log::found_usdt(f),
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
        listen_log::emit(p, pool)?;
        out.push(Ok(()));
    }
    Ok(out)
}
