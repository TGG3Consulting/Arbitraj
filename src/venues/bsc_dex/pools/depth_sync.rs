//! Файл: venues/bsc_dex/pools/depth_sync.rs
//! Что делает: смена порога или полосы глубины на лету (решение 73, ревью #65: живой бот 26.10 — dex.min_pool_liquidity_usd
//! снизили 2000 → 200, у 18 монет пулы стали глубокими в обе стороны, а налоги висели «глубина ниже порога»: вердикт
//! пересчитывался только на событии пула, у тихих пулов событий нет). При изменении dex.min_pool_liquidity_usd,
//! dex.depth_band_pct или dex.depth_hysteresis_pct пачка сразу пересчитывает глубину всех своих пулов по уже
//! прочитанной карте (к узлу не ходит); полоса вышла за прочитанные слова — карта перечитывается вокруг текущего тика;
//! направление стало глубоким — «пул готов» (pools/depth_book.rs on_rise): ждущие глубокого пула налоги и сверка с
//! квотером проверяются сразу. Спящий пул (источник ws), ставший глубоким, просыпается (admit.rs wake) — без
//! переподключения пачки.
//! Кто вызывает: pools/session.rs (ветка изменения настроек).
//! От чего зависит: depth.rs (deep, off_map, touch), admit.rs (wake), load.rs (refresh), registry.rs, listen.rs.
//! Настройки: dex.min_pool_liquidity_usd, dex.depth_band_pct, dex.depth_hysteresis_pct [bsc_dex].

use crate::settings::live::Change;
use crate::storage::journal::Level;

use super::admit::{self, Members};
use super::group::Group;
use super::listen::{End, Pools};
use super::{depth, listen_log, load};

/// Настройки, от которых зависит вердикт глубины.
const DEPTH: [&str; 3] = [
    "dex.min_pool_liquidity_usd",
    "dex.depth_band_pct",
    "dex.depth_hysteresis_pct",
];

/// Касается ли изменение глубины (рассылка отстала — неизвестно, считаем, что да). Ошибок нет.
pub fn touches<E>(changed: &Result<Change, E>) -> bool {
    match changed {
        Ok(c) => DEPTH.contains(&c.name.as_str()),
        Err(_) => true,
    }
}

/// Пересчёт глубины пулов пачки (см. шапку). Ошибки: End — обрыв, сбой узла при пробуждении, ядро закрыло канал.
pub async fn on_change(p: &Pools, g: &mut Group, m: &mut Members) -> Result<(), End> {
    let at = p.heads.latest().map(|h| h.number);
    for i in 0..m.pools.len() {
        let r = m.pools[i].0.reference.clone();
        let Some(e) = p.reg.get(&r) else { continue };
        if !e.valid {
            continue;
        }
        if !m.deep[i] {
            // Спящий пул: по новому порогу глубокий — просыпается (перечитывание и полная подписка).
            if let Some(at) = at
                && depth::deep(p, &r).await
            {
                admit::wake(p, g, m, i, at).await?;
            }
            continue;
        }
        if let Some(at) = at
            && depth::off_map(&p.ctx, &e)
        {
            // Полоса шире прочитанных слов — карта вокруг текущего тика (слова — по новой полосе).
            match load::refresh(&p.ctx, &p.tokens, &e, at).await {
                Ok(x) => {
                    p.reg.put(x.clone());
                    listen_log::emit(p, x.pool)?;
                }
                Err(err) => {
                    let msg = format!(
                        "{} {}: полоса глубины шире прочитанной карты, перечитать не удалось: {}",
                        e.family.title, e.ticker, err.message
                    );
                    p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
                }
            }
            continue;
        }
        // Пересчёт по прочитанной карте; стало глубоким — «пул готов» подаёт depth_book.rs.
        depth::touch(p, &r);
    }
    Ok(())
}
