//! Файл: venues/bsc_dex/pools/depth_sync.rs
//! Что делает: смена порога или полосы глубины на лету (решение 75, ревью #65: живой бот 26.10 — dex.min_pool_liquidity_usd
//! снизили 2000 → 200, у 18 монет пулы стали глубокими, а налоги висели «глубина ниже порога»: вердикт пересчитывался
//! только на событии пула, у тихих пулов событий нет). При изменении dex.min_pool_liquidity_usd, dex.depth_band_pct или
//! dex.depth_hysteresis_pct пачка:
//! 1) сразу пересчитывает вердикт слушаемых пулов по прочитанной карте (к узлу не ходит); стало глубоким — «пул готов»
//!    (pools/depth_book.rs on_rise) — ждущие глубокого пула налоги и сверка проверяются сразу;
//! 2) если нужен узел — полоса шире прочитанных слов (карту перечитать) или есть спящие пулы (источник ws: их состояние
//!    не слушалось, глубина по новому порогу — только после чтения), — на время чтения ВСЯ пачка недостоверна (freeze:
//!    события пачки ждут в канале, котировки по застывшим ценам нет — «не торгуем вслепую»); чтение — одним пакетным
//!    проходом (reload_many.rs, Multicall3, свой свежий блок у каждой группы — ждёт узел HTTP сам); спящие, ставшие
//!    глубокими, — одной переподпиской и одним пакетным чтением на её блоке; затем пачка снова достоверна (thaw), не
//!    прочитавшиеся — недостоверны и перечитываются на новых блоках (reload.rs).
//!
//! Кто вызывает: pools/session.rs (ветка изменения настроек).
//! От чего зависит: depth.rs (deep, mark, off_map, touch), reload_many.rs, reload.rs (Missing), group.rs, registry.rs.
//! Настройки: dex.min_pool_liquidity_usd, dex.depth_band_pct, dex.depth_hysteresis_pct [bsc_dex].

use crate::core::types::PoolRef;
use crate::settings::live::Change;
use crate::storage::journal::Level;

use super::admit::Members;
use super::group::{Group, Topics};
use super::listen::{End, Pools};
use super::reload::{self, Missing};
use super::{depth, http_wait};

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

/// Пачка «застыла»: её достоверные пулы помечаются недостоверными (котировка и проверки их не берут). Возвращает: какие
/// были достоверны (их вернёт thaw). Ошибок нет.
pub fn freeze<'a>(p: &Pools, refs: impl Iterator<Item = &'a PoolRef>) -> Vec<PoolRef> {
    refs.filter(|r| {
        p.reg
            .update(r, |e| std::mem::replace(&mut e.valid, false))
            .unwrap_or(false)
    })
    .cloned()
    .collect()
}

/// Пачка снова достоверна: пулы `frozen`, кроме не прочитавшихся `failed`. Ошибок нет.
pub fn thaw(p: &Pools, frozen: &[PoolRef], failed: &[PoolRef]) {
    for r in frozen.iter().filter(|r| !failed.contains(r)) {
        p.reg.update(r, |e| e.valid = true);
    }
}

/// Пулы `refs` не прочитались — недостоверны (котировка их не берёт), пока reload.rs не перечитает. Ошибок нет.
pub fn fail(p: &Pools, refs: &[PoolRef]) {
    for r in refs {
        p.reg.update(r, |e| e.valid = false);
    }
}

/// Пересчёт глубины пулов пачки (см. шапку). Ошибки: End — обрыв подписки, ядро закрыло канал.
pub async fn on_change(p: &Pools, g: &mut Group, m: &mut Members) -> Result<(), End> {
    let mut need: Vec<usize> = Vec::new();
    for i in 0..m.pools.len() {
        let r = &m.pools[i].0.reference;
        let Some(e) = p.reg.get(r).filter(|e| e.valid) else {
            continue;
        };
        if !m.deep[i] || depth::off_map(&p.ctx, &e) {
            need.push(i);
        } else {
            depth::touch(p, r);
        }
    }
    if need.is_empty() {
        return Ok(());
    }
    let frozen = freeze(p, m.pools.iter().map(|(f, _)| &f.reference));
    let min = p.heads.latest().map_or(0, |h| h.number);
    let (failed, deep_now) = read(p, m, &need, min).await?;
    let woke: Vec<usize> = need
        .iter()
        .copied()
        .filter(|i| !m.deep[*i] && deep_now.contains(i))
        .collect();
    let mut failed = failed;
    if !woke.is_empty() {
        let mut full: Vec<usize> = g.full.as_ref().map(|(_, v)| v.clone()).unwrap_or_default();
        full.extend(&woke);
        full.sort_unstable();
        full.dedup();
        let broken = |e: crate::gateway::errors::GatewayError| End::Broken(e.message);
        g.set(&m.pools, full, Topics::Full).await.map_err(broken)?;
        let at = g.block().await.map_err(broken)?;
        http_wait::http_reached(p, at).await.map_err(broken)?;
        // Подписка «все события» идёт с блока `at` — состояние на нём, события после — в канале пачки.
        let (more, _) = read(p, m, &woke, at).await?;
        failed.extend(more);
        for &i in &woke {
            m.deep[i] = true;
            depth::mark(p, &m.pools[i].0.reference, false);
        }
        let msg = format!(
            "{} пул(ов) стали глубокими по новому порогу глубины — слушаются полностью",
            woke.len()
        );
        p.ctx.log(Level::Info, "dex.pool", msg, None).await;
    }
    let failed_refs: Vec<PoolRef> = failed
        .iter()
        .map(|i| m.pools[*i].0.reference.clone())
        .collect();
    for &i in &failed {
        if !m.missing.iter().any(|x| x.i == i) {
            m.missing.push(Missing { i, tries: 0 });
        }
    }
    // Не прочитался в любом из чтений — недостоверен (первое чтение спящего уже поставило valid на своём блоке:
    // обмены между ним и блоком переподписки потеряны), как admit::wake; перечитывается на новых блоках.
    fail(p, &failed_refs);
    thaw(p, &frozen, &failed_refs);
    Ok(())
}

/// Пакетное чтение пулов `idx` (не раньше блока `min`); спящие после чтения — снова «спят» (их переводит в полную
/// подписку on_change).
/// Возвращает: (не прочитались, глубокие сейчас). Ошибки: End — ядро закрыло канал.
async fn read(
    p: &Pools,
    m: &Members,
    idx: &[usize],
    min: u64,
) -> Result<(Vec<usize>, Vec<usize>), End> {
    let items: Vec<_> = idx
        .iter()
        .map(|&i| (&m.pools[i].0, m.pools[i].1.as_str()))
        .collect();
    let res = reload::load_many(p, &items, min).await?;
    let (mut failed, mut deep) = (Vec::new(), Vec::new());
    for (&i, r) in idx.iter().zip(res) {
        let pr = &m.pools[i].0.reference;
        if r.is_err() {
            failed.push(i);
            continue;
        }
        // Спящий остаётся «спит» (не котируется), пока его не переподпишут на все события и не перечитают на её блоке.
        if !m.deep[i] {
            depth::mark(p, pr, true);
        }
        if depth::deep(p, pr).await {
            deep.push(i);
        }
    }
    if !failed.is_empty() {
        let msg = format!(
            "Пересчёт глубины: {} пул(ов) не прочитаны — недостоверны, перечитываются на новых блоках",
            failed.len()
        );
        p.signals.once(&p.ctx, Level::Warn, "dex.pool", msg).await;
    }
    Ok((failed, deep))
}
