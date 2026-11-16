//! Файл: venues/bsc_dex/pools/admit.rs
//! Что делает: приём пулов в пачку на её соединении и возврат мелкого пула в полную подписку.
//! Приём (начало сессии и пулы, найденные позже): временная подписка «все события» на новые пулы → блок B того же
//! узла WS (всё позже B придёт в подписку) → ждём узел HTTP до B → чтение на B → деление на глубокие и мелкие
//! (depth.rs) → составы постоянных подписок пачки (group.rs) → отписка от временной (если она не стала постоянной).
//! Пробуждение мелкого пула: событие добавления ликвидности на блоке N → чтение на N → глубокий — полная подписка
//! с ним → блок B' → чтение на B' (события после B' придут, до — учтены в чтении) → пул снова котируется.
//! Новые пулы читаются пачками на одном блоке (reload::load_many → load_batch.rs, решение 61).
//! Пул, не прочитанный при приёме, слушается полностью и перечитывается на новых блоках (reload.rs).
//! Решение 339 (ревью t153): пул на паузе после неудач при приёме (переподключение пачки) не читается — сразу в
//! очередь повтора с числом неудач из отметки; новый пул и пул без отметки читаются как раньше. В записях журнала —
//! адрес пула (depth_book::label, как у dex.pool_refresh). При изоляции неудача чтения при приёме — отметка 339
//! (pool_fail::note: журнал на первую неудачу эпизода и смену кода причины), число повторов — из отметки.
//! Кто вызывает: pools/session.rs.
//! От чего зависит: group.rs, depth.rs, depth_book.rs (label), reload.rs, pool_fail.rs (paused, отметки), http_wait.rs,
//! listen.rs (Pools, End).
//! Настройки: через вызываемые (feed.stale_after_ms, dex.min_pool_liquidity_usd, dex.pool_load_retries).

use crate::storage::journal::Level;

use super::depth_book::label;
use super::discover::Found;
use super::group::{Group, Topics};
use super::listen::{End, Pools};
use super::reload::{self, Missing};
use super::{depth, http_wait};

/// Пачка в сессии: пулы, кто из них слушается полностью, кто ждёт повторного чтения.
pub struct Members {
    pub pools: Vec<(Found, String)>,
    /// По индексу пула: true — все события, false — только добавление ликвидности.
    pub deep: Vec<bool>,
    pub missing: Vec<Missing>,
    /// Брошенные пулы (попытки исчерпаны); брошенные из-за незаданной настройки — снова читаются, когда её зададут
    /// (reload::on_settings).
    pub abandoned: Vec<super::reload::Abandoned>,
    /// Чтение состояния уже начиналось (обрыв после него — пулы недостоверны).
    pub loaded: bool,
}

/// Ошибка шлюза → конец сессии с причиной.
fn broken(e: crate::gateway::errors::GatewayError) -> End {
    End::Broken(e.message)
}

/// Что делает: сколько ПОВТОРОВ уже было у пула для очереди повтора (reload.rs Missing.tries — без первого чтения):
/// неудач подряд по отметке 339 минус одна (отметки нет — 0, как раньше). Принимает: окружение, ссылку. Ошибок нет.
fn done_tries(p: &Pools, r: &crate::core::types::PoolRef) -> u64 {
    p.reg
        .fails()
        .get(r)
        .map_or(0, |f| u64::from(f.tries.saturating_sub(1)))
}

/// Принимает в пачку пулы с индексами `new` (см. шапку). Ошибки: End — обрыв, сбой узла или ядро закрыло канал.
pub async fn admit(p: &Pools, g: &mut Group, m: &mut Members, new: Vec<usize>) -> Result<(), End> {
    if new.is_empty() {
        return Ok(());
    }
    m.deep.resize(m.pools.len(), true);
    let list: Vec<&Found> = new.iter().map(|i| &m.pools[*i].0).collect();
    let tmp = g.subscribe(&list, Topics::Full).await.map_err(broken)?;
    let block = g.block().await.map_err(broken)?;
    http_wait::http_reached(p, block).await.map_err(broken)?;
    m.loaded = true;
    // Решение 339 (ревью t153): пул на паузе после неудач (отметка pool_fail.rs) при приёме не читается и записи
    // журнала не даёт — сразу в очередь повтора, читается не раньше паузы (reload.rs retry); число попыток — из
    // отметки (предел dex.pool_load_retries не начинается заново, запись «не прочитано за N попыток» не повторяется).
    // Новый пул и пул без отметки (или 339 выключено) — как раньше.
    let (later, read): (Vec<usize>, Vec<usize>) = new
        .iter()
        .copied()
        .partition(|&i| super::pool_fail::paused(p, &m.pools[i].0.reference));
    for &i in &later {
        let r = &m.pools[i].0.reference;
        p.reg.update(r, |e| e.valid = false);
        m.missing.push(Missing {
            i,
            tries: done_tries(p, r),
        });
        m.deep[i] = true;
        depth::mark(p, r, false);
    }
    // Состояние всех новых пулов — пачками на одном блоке (решение 61), а не по одному.
    let items: Vec<(&super::discover::Found, &str)> = read
        .iter()
        .map(|&i| (&m.pools[i].0, m.pools[i].1.as_str()))
        .collect();
    let results = if items.is_empty() {
        Vec::new()
    } else {
        reload::load_many(p, &items, block).await?
    };
    for (&i, res) in read.iter().zip(results) {
        let (found, ticker) = &m.pools[i];
        if let Err(why) = res {
            p.reg.update(&found.reference, |e| e.valid = false);
            let r = &found.reference;
            // Решение 339 (журнал и приём): при изоляции неудача — отметка с паузой (pool_fail::note: журнал только на
            // первую неудачу эпизода и смену кода причины, а не на каждое переподключение), попытки — из отметки;
            // StaleData — как раньше (без паузы). Не задана dex.pool_fail_pause_ms — запись на каждую неудачу.
            let noted = !super::stale_read::is_stale(&why)
                && super::pool_fail::note(p, r, found.family.title, ticker, &why).await;
            if !noted {
                let msg = format!(
                    "{} {ticker} ({}): состояние пула не прочитано: {} — повтор на новых блоках",
                    found.family.title,
                    label(r),
                    why.message
                );
                p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
            }
            // Ждёт незаданную настройку — «застрял»: сводку первого прохода не держит (loading.rs).
            p.loading.stuck(p, r, why.setting.is_some());
            m.missing.push(Missing {
                i,
                tries: done_tries(p, r),
            });
            m.deep[i] = true;
            depth::mark(p, &found.reference, false);
            continue;
        }
        let r = &found.reference;
        m.deep[i] = depth::deep(p, r).await;
        depth::mark(p, r, !m.deep[i]);
    }
    let full: Vec<usize> = (0..m.pools.len()).filter(|i| m.deep[*i]).collect();
    let thin: Vec<usize> = (0..m.pools.len()).filter(|i| !m.deep[*i]).collect();
    // Начало сессии без мелких пулов: временная подписка и есть постоянная — без второй подписки.
    if g.full.is_none() && full == new {
        g.full = Some((tmp, new));
    } else {
        g.set(&m.pools, full, Topics::Full).await.map_err(broken)?;
        g.release(tmp).await.map_err(broken)?;
    }
    g.set(&m.pools, thin, Topics::Liquidity)
        .await
        .map_err(broken)?;
    Ok(())
}

/// Мелкий пул `i` получил добавление ликвидности на блоке `block` (см. шапку).
/// Ошибки: End — обрыв, сбой узла или ядро закрыло канал. Пул не прочитался — остаётся мелким (запись в журнал).
pub async fn wake(
    p: &Pools,
    g: &mut Group,
    m: &mut Members,
    i: usize,
    block: u64,
) -> Result<(), End> {
    http_wait::http_reached(p, block).await.map_err(broken)?;
    let (found, ticker) = m.pools[i].clone();
    let title = found.family.title;
    if let Err(why) = reload::load_one(p, &found, &ticker, block).await? {
        let msg = format!(
            "{title} {ticker} ({}): добавление ликвидности, пул не прочитан: {}",
            label(&found.reference),
            why.message
        );
        p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
        return Ok(());
    }
    if !depth::deep(p, &found.reference).await {
        return Ok(());
    }
    let mut full: Vec<usize> = g.full.as_ref().map(|(_, v)| v.clone()).unwrap_or_default();
    full.push(i);
    full.sort_unstable();
    g.set(&m.pools, full, Topics::Full).await.map_err(broken)?;
    let at = g.block().await.map_err(broken)?;
    http_wait::http_reached(p, at).await.map_err(broken)?;
    if let Err(why) = reload::load_one(p, &found, &ticker, at).await? {
        p.reg.update(&found.reference, |e| e.valid = false);
        m.missing.push(Missing { i, tries: 0 });
        let msg = format!(
            "{title} {ticker} ({}): состояние пула не прочитано: {} — повтор на новых блоках",
            label(&found.reference),
            why.message
        );
        p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
    }
    m.deep[i] = true;
    depth::mark(p, &found.reference, false);
    let msg = format!(
        "{title} {ticker}: пул стал глубоким (ликвидность выросла или порог глубины снижен) — снова слушается полностью"
    );
    p.ctx.log(Level::Info, "dex.pool", msg, None).await;
    Ok(())
}
