//! Файл: venues/bsc_dex/pools/admit.rs
//! Что делает: приём пулов в пачку на её соединении и возврат мелкого пула в полную подписку.
//! Приём (начало сессии и пулы, найденные позже): временная подписка «все события» на новые пулы → блок B того же
//! узла WS (всё позже B придёт в подписку) → ждём узел HTTP до B → чтение на B → деление на глубокие и мелкие
//! (depth.rs) → составы постоянных подписок пачки (group.rs) → отписка от временной (если она не стала постоянной).
//! Пробуждение мелкого пула: событие добавления ликвидности на блоке N → чтение на N → глубокий — полная подписка
//! с ним → блок B' → чтение на B' (события после B' придут, до — учтены в чтении) → пул снова котируется.
//! Пул, не прочитанный при приёме, слушается полностью и перечитывается на новых блоках (reload.rs).
//! Кто вызывает: pools/session.rs.
//! От чего зависит: group.rs, depth.rs, reload.rs, http_wait.rs, listen.rs (Pools, End).
//! Настройки: через вызываемые (feed.stale_after_ms, dex.min_pool_liquidity_usd, dex.pool_load_retries).

use crate::storage::journal::Level;

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
    /// Чтение состояния уже начиналось (обрыв после него — пулы недостоверны).
    pub loaded: bool,
}

/// Ошибка шлюза → конец сессии с причиной.
fn broken(e: crate::gateway::errors::GatewayError) -> End {
    End::Broken(e.message)
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
    for &i in &new {
        let (found, ticker) = &m.pools[i];
        if let Err(why) = reload::load_one(p, found, ticker, block).await? {
            p.reg.update(&found.reference, |e| e.valid = false);
            let msg = format!(
                "{} {ticker}: состояние пула не прочитано: {why} — повтор на новых блоках",
                found.family.title
            );
            p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
            m.missing.push(Missing { i, tries: 0 });
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
        let msg = format!("{title} {ticker}: добавление ликвидности, пул не прочитан: {why}");
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
            "{title} {ticker}: состояние пула не прочитано: {why} — повтор на новых блоках"
        );
        p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
    }
    m.deep[i] = true;
    depth::mark(p, &found.reference, false);
    let msg = format!(
        "{title} {ticker}: ликвидность выросла до dex.min_pool_liquidity_usd — пул снова слушается полностью"
    );
    p.ctx.log(Level::Info, "dex.pool", msg, None).await;
    Ok(())
}
