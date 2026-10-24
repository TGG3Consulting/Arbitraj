//! Файл: venues/bsc_dex/pools/reload.rs
//! Что делает: чтение состояния пула пачки на блоке с отправкой ядру (load_one) и повтор чтения пулов, которые не
//! прочитались (сбой узла): на каждом новом блоке (опрос блоков node/heads.rs), не больше dex.pool_load_retries раз. Журнал: одна запись
//! «не прочитан», одна — «прочитан с попытки N» или «так и не прочитан за N попыток».
//! Многие пулы сразу (приём пачки, повтор) — load_many: пачками через Multicall3 (load_batch.rs, решение 61).
//! Кто вызывает: pools/session.rs, pools/admit.rs.
//! От чего зависит: pools/load.rs, listen_log.rs (emit, note_refill, found_usdt), registry.rs, progress.rs.
//! Настройки: dex.pool_load_retries [bsc_dex].

use crate::storage::journal::Level;

use super::discover::Found;
use super::listen::{End, Pools};
use super::load_batch::{self, Want};
use super::{listen_log, load};

/// Пул, ждущий повторного чтения: индекс в пачке и сколько повторов уже было.
pub struct Missing {
    pub i: usize,
    pub tries: u64,
}

/// Повтор чтения ждущих пулов на блоке `at`. Ошибки: End — ядро закрыло канал.
pub async fn retry(
    p: &Pools,
    pools: &[(Found, String)],
    missing: &mut Vec<Missing>,
    at: u64,
) -> Result<(), End> {
    let limit = match p.ctx.int("dex.pool_load_retries") {
        Ok(n) => n,
        Err(e) => {
            p.signals
                .once(
                    &p.ctx,
                    Level::Error,
                    "dex.pool",
                    format!("Повтор чтения пулов невозможен: {}", e.message),
                )
                .await;
            missing.clear();
            return Ok(());
        }
    };
    let mut keep = Vec::new();
    let waiting = std::mem::take(missing);
    let items: Vec<(&Found, &str)> = waiting
        .iter()
        .map(|m| (&pools[m.i].0, pools[m.i].1.as_str()))
        .collect();
    let results = load_many(p, &items, at).await?;
    for (mut m, res) in waiting.into_iter().zip(results) {
        let (found, ticker) = &pools[m.i];
        m.tries += 1;
        let title = found.family.title;
        match res {
            Ok(()) => {
                let msg = format!(
                    "{title} {ticker}: состояние пула прочитано с попытки {}",
                    m.tries + 1
                );
                p.ctx.log(Level::Info, "dex.pool", msg, None).await;
                p.signals.pool_ready();
            }
            Err(why) if m.tries >= limit => {
                let msg = format!(
                    "{title} {ticker}: состояние пула так и не прочитано за {} попыток: {why} — недостоверен до переподключения",
                    m.tries + 1
                );
                p.ctx.log(Level::Error, "dex.pool", msg, None).await;
            }
            Err(_) => keep.push(m),
        }
    }
    *missing = keep;
    Ok(())
}

/// Чтение одного пула на блоке и отправка ядру. Возвращает: Ok(Ok) — прочитан; Ok(Err(причина)) — нет.
/// Ошибки: End — ядро закрыло канал.
pub async fn load_one(
    p: &Pools,
    found: &Found,
    ticker: &str,
    block: u64,
) -> Result<Result<(), String>, End> {
    let res = match p.reg.get(&found.reference) {
        Some(e) => load::refresh(&p.ctx, &p.tokens, &e, block).await,
        None => {
            load::entry(
                &p.ctx,
                &p.tokens,
                found,
                ticker,
                listen_log::found_usdt(found),
                block,
            )
            .await
        }
    };
    match res {
        Ok(e) => {
            listen_log::note_refill(p, &e);
            let pool = e.pool.clone();
            p.reg.put(e);
            listen_log::emit(p, pool).map(Ok)
        }
        Err(e) => Ok(Err(e.message)),
    }
}

/// Чтение многих пулов на одном блоке пачками (load_batch.rs, решение 61) и отправка ядру прочитанных.
/// Принимает: (пул, тикер) по порядку, блок. Возвращает: по каждому Ok — прочитан, Err(причина) — нет (как load_one).
/// Ошибки: End — ядро закрыло канал.
pub async fn load_many(
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
