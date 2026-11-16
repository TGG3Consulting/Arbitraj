//! Файл: venues/bsc_dex/check/quoter_reprobe.rs
//! Что делает: решение 335 (задача t156, пункт а; переделано по ревью) — перемер влияния крючка перед паузой сверки с
//! квотером. Сверка стороны пула с крючком группы B/C на блоке H разошлась больше dex.quoter_max_diff_pct
//! (Cmp::Pause), а вычтенное влияние крючка измерено раньше и на другом состоянии (check/hooks_probe.rs). При
//! hooks.reprobe_before_pause = да:
//! - АСИММЕТРИЯ: перемер — только при расхождении в безопасную сторону (наш выход НИЖЕ квотера, reprobe_rule::safe).
//!   Наш выход ВЫШЕ квотера (опасная сторона) — пауза как до решения 335, с пометкой reprobe_rule::DANGER;
//! - лимит группы hooks.max_impact_b_pct / hooks.max_impact_c_pct обязателен: не задан — перемер не применяется,
//!   пауза держится (в том числе для продажи выхода, где quote/select.rs лимит не применяет); влияние стороны после
//!   перемера больше лимита — пауза с причиной, итог не пишется и ядру не уходит (алерта о снятии нет);
//! - влияние перемеряется НА ТОМ ЖЕ блоке H по тому же свежему состоянию (hooks_probe::measure_on; один раз на пул и
//!   блок в сверке пула — ревью Н4), берётся только для сверяемой стороны (reprobe_rule::only_side), пишется через
//!   check/hook_store.rs put_head (не затирает более новый блок) и уходит ядру (PoolCheck); сторона сверяется на H
//!   ещё раз. Пауза — если расходится и после перемера; перемер не удался — пауза прежняя с пометкой.
//!
//! Журнал — вид dex.hook_reprobe (было → стало, итог). Отрицательное влияние по-прежнему не прибавляется к нашему
//! выходу (quote/quoter.rs less_hook, решение 261) — сторона остаётся на паузе с пометкой «влияние отрицательное».
//! Не задана или «нет» — итог сверки как до решения 335. Действует на все виды пулов с крючком B/C (Infinity CL,
//! Infinity Bin, Uniswap V4 — у остальных видов Pool.hooks = None).
//! Кто вызывает: check/quoter_verify.rs (verify).
//! От чего зависит: check/reprobe_rule.rs, check/hooks_probe.rs, check/hook_store.rs, check/quoter_compare.rs,
//! pools/listen.rs (Pools), node/call.rs (block_tag), gateway/events.rs (PoolCheck).
//! Настройки: hooks.reprobe_before_pause [bsc_dex] — необязательная; hooks.max_impact_b_pct, hooks.max_impact_c_pct
//! (docs/SETTINGS.md).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup, HookImpact, Side};
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::node::call;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::quoter_compare::{self, Cmp};
use super::reprobe_rule::{DANGER, merge, note, only_side, safe, side_pct};
use super::{hook_store, hooks_probe};

/// Настройка: перемер влияния крючка на блоке сверки перед паузой (решение 335, необязательна).
pub const REPROBE: &str = "hooks.reprobe_before_pause";

/// Что сверялось на блоке: свежая запись пула на H, H, сторона, токен входа и сумма (минимальные единицы), влияние
/// обеих сторон, с которым шла первая сверка, и перемер этого пула на блоке (общий для сторон, ревью Н4).
pub struct Try<'a> {
    pub fresh: &'a Entry,
    pub block: u64,
    pub side: Side,
    pub token_in: Address,
    pub amount: U256,
    pub old: Option<HookImpact>,
    pub cache: &'a mut Option<(u64, HookImpact)>,
}

/// Что делает: перемер перед паузой (см. шапку). Принимает: пулы, запись из памяти, что сверялось, итог первой
/// сверки. Возвращает: итог стороны — первый как есть (не пауза, нет крючка B/C, настройка не задана), первый с
/// пометкой (опасная сторона, лимит, перемер не удался) или после перемера. Ошибки: только от первой сверки.
pub async fn again(
    p: &Pools,
    e: &Entry,
    t: Try<'_>,
    got: GatewayResult<Cmp>,
) -> GatewayResult<Cmp> {
    let first = match got {
        Ok(c @ Cmp::Pause { .. }) => c,
        other => return other,
    };
    let group = match e.pool.hooks.as_ref().map(|h| h.group) {
        Some(g) if g != HookGroup::A => g,
        _ => return Ok(first),
    };
    if !p.ctx.flag(REPROBE).unwrap_or(false) {
        return Ok(first);
    }
    let Some(old) = t.old.clone() else {
        return Ok(first);
    };
    if !safe(&first) {
        return Ok(note(first, DANGER));
    }
    let (name, limit) = match limit(p, group) {
        Ok(v) => v,
        Err(why) => return Ok(note(first, &format!("перемер не применяется: {why}"))),
    };
    // Ревью Н4: обе стороны на паузе — перемер на этом блоке один раз на пул.
    let cached = t
        .cache
        .as_ref()
        .filter(|(b, _)| *b == t.block)
        .map(|(_, i)| i.clone());
    let i = match cached {
        Some(i) => i,
        None => {
            let tag = call::block_tag(t.block);
            let i = match hooks_probe::measure_on(&p.ctx, &p.tokens, &p.reg, t.fresh, &tag).await {
                Ok(Ok(i)) => i,
                Ok(Err(why)) => return Ok(note(first, &failed(t.block, &why))),
                Err(err) => return Ok(note(first, &failed(t.block, &err.message))),
            };
            *t.cache = Some((t.block, i.clone()));
            i
        }
    };
    let pct = side_pct(&i, t.side);
    if pct > limit {
        let add = format!(
            "влияние крючка на сторону после перемера {}% больше {name} ({limit}%)",
            pct.round_dp(4)
        );
        log(p, e, &t, &add).await;
        return Ok(note(first, &add));
    }
    let merged = only_side(&old, &i, t.side);
    let pc = hooks_probe::pool_check(&p.ctx, e, group, CheckState::Known(merged.clone()));
    if !hook_store::put_head(&p.checks, &e.pool.reference, t.block, &pc, None) {
        let add = "влияние крючка уже измерено на более новом блоке — перемер не применяется";
        return Ok(note(first, add));
    }
    let _ = p.out.send(MarketEvent::PoolCheck(pc)).await;
    let second = quoter_compare::compare(&p.ctx, t.fresh, t.token_in, t.amount, pct, t.block).await;
    let res = merge(first, second, &merged, t.side, t.block);
    let msg = format!(
        "было покупка {}% / продажа {}%, перемер стороны {}% → {}",
        old.buy_pct.round_dp(4),
        old.sell_pct.round_dp(4),
        pct.round_dp(4),
        if matches!(res, Cmp::Pause { .. }) {
            "пауза"
        } else {
            "сверка без паузы"
        }
    );
    log(p, e, &t, &msg).await;
    Ok(res)
}

/// Пометка «перемер не удался». Ошибок нет.
fn failed(block: u64, why: &str) -> String {
    format!("перемер влияния крючка на блоке {block} не удался: {why}")
}

/// Лимит влияния группы (как quote/select.rs usable). Принимает: пулы, группу. Возвращает: имя настройки и значение.
/// Ошибки (текст): не задана или не читается.
fn limit(p: &Pools, group: HookGroup) -> Result<(&'static str, Decimal), String> {
    let name = if group == HookGroup::B {
        "hooks.max_impact_b_pct"
    } else {
        "hooks.max_impact_c_pct"
    };
    p.ctx
        .settings
        .decimal(name, At::default())
        .map(|v| (name, v))
        .map_err(|e| format!("{name}: {}", e.message))
}

/// Запись перемера в журнал dex.hook_reprobe. Принимает: пулы, запись, что сверялось, текст. Ошибок нет.
async fn log(p: &Pools, e: &Entry, t: &Try<'_>, text: &str) {
    let msg = format!(
        "{} {} ({}): перед паузой влияние крючка перемерено на блоке {}: {text}",
        e.family.title,
        e.ticker,
        super::quoter_pause::side_name(t.side),
        t.block
    );
    p.ctx.log(Level::Info, "dex.hook_reprobe", msg, None).await;
}
