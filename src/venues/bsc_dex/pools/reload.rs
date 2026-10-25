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

/// Брошенный пул: индекс в пачке и незаданная настройка, из-за которой он не прочитался (None — другая причина:
/// такой пул ждёт переподключения пачки).
#[derive(Debug, Clone)]
pub struct Abandoned {
    pub i: usize,
    pub setting: Option<String>,
}

/// Брошенные из-за настройки `name` пулы снова ждут чтения (её задали): попытки — заново, в «загружаются» — снова.
/// Остальные брошенные и ждущие не трогаются. Принимает: пулы пачки, брошенные, имя заданной настройки.
/// Возвращает: вернувшиеся. Ошибок нет.
pub fn revive(
    p: &Pools,
    pools: &[(Found, String)],
    abandoned: &mut Vec<Abandoned>,
    name: &str,
) -> Vec<Missing> {
    let (back, keep): (Vec<Abandoned>, Vec<Abandoned>) = std::mem::take(abandoned)
        .into_iter()
        .partition(|a| a.setting.as_deref() == Some(name));
    *abandoned = keep;
    p.loading.add(back.iter().map(|a| &pools[a.i].0));
    back.into_iter()
        .map(|a| Missing { i: a.i, tries: 0 })
        .collect()
}

/// Изменилась настройка: если её задали (новое значение есть), пулы, брошенные из-за неё, — снова в чтение, сразу
/// попытка на последнем блоке только для них (попытки обычных ждущих не тратятся); не прочитались — к ждущим.
/// Принимает: окружение, пулы пачки, ждущие, брошенные, изменение. Ошибки: End — ядро закрыло канал.
pub async fn on_settings(
    p: &Pools,
    pools: &[(Found, String)],
    missing: &mut Vec<Missing>,
    abandoned: &mut Vec<Abandoned>,
    change: &crate::settings::live::Change,
) -> Result<(), End> {
    if change.new.is_none() {
        return Ok(());
    }
    let mut back = revive(p, pools, abandoned, &change.name);
    if back.is_empty() {
        return Ok(());
    }
    p.loading.publish(p).await;
    if let Some(h) = p.heads.latest() {
        retry(p, pools, &mut back, abandoned, h.number).await?;
    }
    missing.extend(back);
    Ok(())
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
    abandoned: &mut Vec<Abandoned>,
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
                // Брошен из-за незаданной настройки — вернётся, когда её зададут (on_settings).
                let setting = crate::settings::read::missing_name(&why).map(String::from);
                abandoned.push(Abandoned { i: m.i, setting });
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
