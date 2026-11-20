//! Файл: venues/bsc_dex/check/hooks_head.rs
//! Что делает: проба влияния крючка пула V4 группы B/C в цикле крючков (check/cycle.rs) — решение 335, после
//! «335 (В1)» это единственный путь (настройки hooks.probe_same_block нет): наша математика и квотер — на ОДНОМ
//! блоке. Голова H — один eth_blockNumber, пул читается заново на H тем же загрузчиком, что у сверки с квотером
//! (check/quoter_block.rs at_head, в реестр не пишется) — в том числе спящий пул (его обмены не слушаются и цена в
//! памяти стоит, pools/depth.rs: память в пробе не участвует), математика без крючка — по этому состоянию, квотер —
//! eth_call на тот же H (check/hooks_probe.rs measure_plan). Решение «335 (лесенка влияния)»: на нескольких суммах по
//! каждой стороне (check/hook_points.rs: от signal.probe_usd до min(money.max_per_trade_usd, ликвидность стороны),
//! hooks.impact_points точек) — все вызовы квотера одним пакетом на H; итог — лесенка «вход → влияние». Ревью
//! второго прохода: суммы — по записи на H (Н12); после первого замера (⌈n/2⌉ точек) — второй пакет на том же H с
//! точками в отрезках с наибольшим скачком влияния (Н3, check/hooks_refine.rs; не удался — итог первого);
//! hooks.impact_points не задана — одна точка на check.probe_usdt и одно число на все суммы, как до лесенки (Н6), с
//! записью в журнал один раз (dex.check, Warn).
//! Узел не отдал голову или состояние H, сбой связи — влияние
//! не перемеряется, действует прежнее (если прежнего нет — «не измерено» с причиной; ревью Н1: прежнее старше
//! hooks.keep_impact_max_age_s — «не измерено: устарело»; не задана — прежнее без срока).
//! Итог на блоке пишется через check/hook_store.rs put_head (более старый блок не затирает более новый, ревью Н3).
//! requote (решение 335 (в), с «335 (В1)» — всегда, без настройки): после удачного перемера влияния пул, у которого
//! сторона на паузе сверки с квотером, сверяется сразу (pools/progress.rs requote → check/quoter_cycle.rs), а не через
//! dex.quoter_check_s; только после удачной пробы (итог «измерено», ревью Н5).
//! Действует на все виды пулов с крючком: крючок есть только у пулов менеджеров V4 — PancakeSwap Infinity CL,
//! Infinity Bin и Uniswap V4 (pools/discover_v4.rs from_key); у остальных видов Pool.hooks = None (разбор —
//! docs/notes/t156-hook-pools-quoter-pause-2026-11-16.md).
//! Кто вызывает: check/cycle.rs (one).
//! От чего зависит: check/hooks_probe.rs, check/hook_points.rs, check/hooks_refine.rs, check/hook_store.rs, check/quoter_block.rs, check/quoter_compare.rs (node_fault), check/state.rs,
//! pools/listen.rs (Pools), pools/progress.rs (requote), node/call.rs (block_tag).
//! Настройки: hooks.keep_impact_max_age_s [bsc_dex] — необязательная (docs/SETTINGS.md); через check/hook_points.rs —
//! hooks.impact_points, check.probe_usdt, signal.probe_usd, money.max_per_trade_usd, dex.depth_band_pct.

use std::sync::Mutex;

use chrono::Utc;

use crate::core::types::{CheckState, HookGroup, PoolCheck, PoolRef, Side};
use crate::storage::journal::Level;

use super::super::node::call;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::hook_points;
use super::hook_store;
use super::hooks_probe;
use super::hooks_refine;
use super::quoter_block::{self, Head};
use super::quoter_compare::node_fault;
use super::quoter_verify;

/// Итог пробы для цикла крючков.
#[derive(Debug, Clone)]
pub enum Probe {
    /// У пула нет крючка группы B/C — проверять нечего.
    NoHook,
    /// Влияние не перемерено (узел не отдал блок): прежний итог остаётся, причина — в журнал.
    Keep(String),
    /// Итог пробы, имя незаданной настройки, если проба не прошла из-за неё (решение 75), и блок измерения (None —
    /// блок не прочитан, итог — отказ без влияния: пишется в PoolFacts.hook; Some(H) — через hook_store::put_head).
    Got(Box<PoolCheck>, Option<String>, Option<u64>),
}

/// Что делает: проба влияния крючка пула на одном блоке (см. шапку).
/// Принимает: пулы, запись пула из памяти (только ключ и группа — состояние читается на H). Возвращает: Probe.
/// Ошибок нет: сбой — причина в итоге.
pub async fn probe(p: &Pools, e: &Entry) -> Probe {
    let Some(group) = e.pool.hooks.as_ref().map(|h| h.group) else {
        return Probe::NoHook;
    };
    if group == HookGroup::A {
        return Probe::NoHook;
    }
    let failed = |reason: String, wait: Option<String>, block: Option<u64>| {
        let pc = hooks_probe::pool_check(&p.ctx, e, group, CheckState::Failed { reason });
        Probe::Got(Box::new(pc), wait, block)
    };
    let (block, fresh) = match quoter_block::at_head(p, e).await {
        Head::Ready { block, entry } => (block, entry),
        Head::NoBlock(why) => return keep(p, e, group, why),
        Head::Fail((why, setting)) => return failed(why, setting, None),
    };
    let tag = call::block_tag(block);
    // Решение «335 (лесенка влияния)»: суммы пробы по сторонам — из данных (check/hook_points.rs), все вызовы квотера —
    // одним пакетом на H (hooks_probe::measure_plan).
    let dec = match quoter_verify::usdt_decimals(p).await {
        Ok(d) => d,
        Err(why) => return keep(p, e, group, why),
    };
    let sides = match hook_points::sides(p, &fresh, dec) {
        Ok(v) => v,
        Err((why, setting)) => return failed(why, setting, None),
    };
    if sides.flat && first_flat(p.ctx.place.id) {
        let msg = format!(
            "{} не задана — влияние крючков B/C меряется одной точкой на check.probe_usdt и действует на все суммы, \
             как до лесенки (решение «335 (лесенка влияния)», ревью Н6); проба перед сделкой — на сумме сделки",
            hook_points::POINTS
        );
        p.ctx.log(Level::Warn, "dex.check", msg, None).await;
    }
    let (buy, sell) = (&sides.buy, &sides.sell);
    let plan = match hooks_probe::ladder(&p.ctx, &p.tokens, &p.reg, &fresh, (buy, sell)).await {
        Ok(Ok(plan)) => plan,
        Ok(Err(why)) => return failed(why, None, Some(block)),
        Err(err) => return failed(err.message, err.setting, None),
    };
    // Ревью 3 (Б4): отказ квотера на точке — эта точка и большие суммы стороны; пул «не прошла» — только обе стороны.
    let got = hooks_probe::measure_plan(&p.ctx, &fresh, &plan, &tag).await;
    match hooks_probe::settled(got, &plan) {
        Ok(Ok(i)) => {
            let i = hooks_refine::refined(p, &fresh, &sides, i, &tag).await;
            let pc = hooks_probe::pool_check(&p.ctx, e, group, CheckState::Known(i));
            Probe::Got(Box::new(pc), None, Some(block))
        }
        Ok(Err(reason)) => failed(reason, None, Some(block)),
        Err(err) if node_fault(&err) => keep(p, e, group, format!("блок {block}: {}", err.message)),
        Err(err) => failed(err.message, err.setting, Some(block)),
    }
}

/// Площадки DEX, у которых запись «hooks.impact_points не задана» уже в журнале (один раз за запуск на площадку —
/// решение 435, 433-и).
static FLAT_SAID: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Что делает: первый ли раз запись «не задана» у площадки — запоминает. Принимает: площадку DEX. Ошибок нет.
fn first_flat(venue: &'static str) -> bool {
    let mut said = FLAT_SAID.lock().unwrap_or_else(|p| p.into_inner());
    if said.contains(&venue) {
        return false;
    }
    said.push(venue);
    true
}

/// Что делает: «не перемерено» — прежний итог остаётся (Keep); прежнего итога нет — «не измерено» с причиной
/// (иначе сверка ждала бы влияния до следующего полного прохода молча). Ревью Н1: hooks.keep_impact_max_age_s задана
/// и прежний итог (на блоке, иначе обычный) старше её — «не измерено: устарело» (пул исключается, как до решения 335
/// при сбое пробы); не задана — прежний итог без срока. Принимает: пулы, запись, группу, причину. Возвращает: Probe.
/// Ошибок нет.
fn keep(p: &Pools, e: &Entry, group: HookGroup, why: String) -> Probe {
    let f = p.checks.pool(&e.pool.reference);
    let at = match (&f.hook_head, &f.hook) {
        (Some((_, pc)), _) | (None, Some(pc)) => Some(pc.checked_at),
        (None, None) => None,
    };
    let reason = match (at, p.ctx.int(hook_store::KEEP_AGE)) {
        (Some(at), Ok(max)) if hook_store::stale(at, Utc::now(), max) => format!(
            "влияние крючка устарело (последнее измерение старше {} = {max} с), перемерить на одном блоке нельзя: {why}",
            hook_store::KEEP_AGE
        ),
        (Some(_), _) => return Probe::Keep(why),
        (None, _) => format!("влияние крючка не измерено на одном блоке: {why}"),
    };
    let pc = hooks_probe::pool_check(&p.ctx, e, group, CheckState::Failed { reason });
    Probe::Got(Box::new(pc), None, None)
}

/// Что делает: решение 335 (в) — при удачной пробе (итог «измерено», ревью Н5) и стороне пула на паузе сверки с
/// квотером пул ставится на сверку сразу (pools/progress.rs requote). С «335 (В1)» — всегда (настройки
/// hooks.requote_after_probe нет). Принимает: пулы, пул, удалась ли проба. Возвращает: поставлен ли. Ошибок нет.
pub fn requote(p: &Pools, r: &PoolRef, known: bool) -> bool {
    if !known {
        return false;
    }
    let q = p.checks.pool(r).quoter;
    if !(q.paused(Side::Buy) || q.paused(Side::Sell)) {
        return false;
    }
    p.signals.requote(r.clone());
    true
}
