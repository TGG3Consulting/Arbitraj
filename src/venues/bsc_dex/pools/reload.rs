//! Файл: venues/bsc_dex/pools/reload.rs
//! Что делает: чтение состояния пула пачки на блоке с отправкой ядру (load_one) и повтор чтения пулов, которые не
//! прочитались (сбой узла): на каждом новом блоке (опрос блоков node/heads.rs), не больше dex.pool_load_retries раз. Журнал: одна запись
//! «не прочитан», одна — «прочитан с попытки N» или «так и не прочитан за N попыток».
//! Многие пулы сразу (приём пачки, повтор) — load_many: пачками через Multicall3 (load_batch.rs, решение 61).
//! Кто вызывает: pools/session.rs, pools/admit.rs.
//! Прочитанный (или окончательно не прочитанный) пул уходит из «загружается» (loading.rs).
//! От чего зависит: pools/load.rs, loading.rs, listen_log.rs (emit, note_refill, found_usdt), registry.rs, progress.rs.
//! Настройки: dex.pool_load_retries [bsc_dex].

use crate::storage::journal::Level;

use super::discover::Found;
use super::listen::{End, Pools};
use super::{listen_log, load};

/// Брошенные пулы снова ждут чтения (изменились настройки: могла появиться недостающая — dex.load_batch и т.п.):
/// попытки — заново, в «загружаются» — снова. Принимает: пулы пачки, брошенные, ждущие. Ошибок нет.
pub fn revive(
    p: &Pools,
    pools: &[(Found, String)],
    abandoned: &mut Vec<usize>,
    missing: &mut Vec<Missing>,
) {
    let back: Vec<usize> = std::mem::take(abandoned);
    p.loading.add(back.iter().map(|&i| &pools[i].0));
    missing.extend(back.into_iter().map(|i| Missing { i, tries: 0 }));
}

/// Настройки изменились: брошенные пулы — снова в чтение (revive) и сразу попытка на последнем блоке.
/// Принимает: окружение, пулы пачки, ждущие, брошенные. Ошибки: End — ядро закрыло канал.
pub async fn on_settings(
    p: &Pools,
    pools: &[(Found, String)],
    missing: &mut Vec<Missing>,
    abandoned: &mut Vec<usize>,
) -> Result<(), End> {
    revive(p, pools, abandoned, missing);
    p.loading.publish(p).await;
    match p.heads.latest() {
        Some(h) if !missing.is_empty() => retry(p, pools, missing, abandoned, h.number).await,
        _ => Ok(()),
    }
}

/// Пул, ждущий повторного чтения: индекс в пачке и сколько повторов уже было.
pub struct Missing {
    pub i: usize,
    pub tries: u64,
}

/// Повтор чтения ждущих пулов на блоке `at`. Исчерпавшие попытки — в `abandoned` (брошены до переподключения или до
/// изменения настроек — session.rs revive). Не задана dex.pool_load_retries — ждущие остаются ждать (повтор после её
/// появления). Ошибки: End — ядро закрыло канал.
pub async fn retry(
    p: &Pools,
    pools: &[(Found, String)],
    missing: &mut Vec<Missing>,
    abandoned: &mut Vec<usize>,
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
                    "{title} {ticker}: состояние пула так и не прочитано за {} попыток: {why} — недостоверен до переподключения или изменения настроек",
                    m.tries + 1
                );
                p.ctx.log(Level::Error, "dex.pool", msg, None).await;
                // Дальше пул «недостоверен», а не «загружается» (loading.rs).
                p.loading.done(&found.reference);
                p.loading.publish(p).await;
                abandoned.push(m.i);
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
            p.loading.done(&found.reference);
            p.loading.publish(p).await;
            listen_log::emit(p, pool).map(Ok)
        }
        Err(e) => Ok(Err(e.message)),
    }
}

pub use super::reload_many::load_many;
