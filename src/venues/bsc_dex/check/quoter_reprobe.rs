//! Файл: venues/bsc_dex/check/quoter_reprobe.rs
//! Что делает: решение 335 (задача t156, пункт а) — перемер влияния крючка перед паузой сверки с квотером. Сверка
//! стороны пула с крючком группы B/C на блоке H разошлась больше dex.quoter_max_diff_pct (Cmp::Pause), а влияние
//! крючка, которое из нашего выхода вычиталось, было измерено раньше и на другом состоянии (check/hooks_probe.rs).
//! При hooks.reprobe_before_pause = да влияние крючка перемеряется НА ТОМ ЖЕ блоке H по тому же свежему состоянию пула
//! (hooks_probe::measure_on), новый итог пишется в проверки пула и уходит ядру (PoolCheck — котировки берут его), и
//! сторона сверяется на H ещё раз с новым влиянием. Пауза — только если расходится и после перемера; перемер не удался
//! — пауза прежняя, причина перемера дописана. Журнал — вид dex.hook_reprobe (было → стало, итог).
//! Отрицательное влияние (квотер с крючком даёт больше нашей математики без крючка) по-прежнему не прибавляется к
//! нашему выходу (quote/quoter.rs less_hook, решение 261) — такая сторона остаётся на паузе, в причине — пометка
//! «влияние отрицательное» (вопрос Тиграну t156 (г), docs/TASKS.md).
//! Не задана или «нет» — итог сверки как до решения 335. Действует на все виды пулов с крючком B/C (Infinity CL,
//! Infinity Bin, Uniswap V4 — у остальных видов крючка нет, Pool.hooks = None).
//! Кто вызывает: check/quoter_verify.rs (verify).
//! От чего зависит: check/hooks_probe.rs, check/quoter_compare.rs, check/state.rs, pools/listen.rs (Pools),
//! node/call.rs (block_tag), gateway/events.rs (PoolCheck).
//! Настройки: hooks.reprobe_before_pause [bsc_dex] — необязательная (docs/SETTINGS.md).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup, HookImpact, Side};
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::node::call;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::hooks_probe;
use super::quoter_compare::{self, Cmp};

/// Настройка: перемер влияния крючка на блоке сверки перед паузой (решение 335, необязательна).
pub const REPROBE: &str = "hooks.reprobe_before_pause";

/// Пометка причины: влияние на сторону отрицательное, вычитается 0 (вопрос t156 (г)).
pub const NEGATIVE: &str = "влияние крючка на эту сторону отрицательное";

/// Что сверялось на блоке: свежая запись пула на H, H, сторона, токен входа и сумма (минимальные единицы).
pub struct Try<'a> {
    pub fresh: &'a Entry,
    pub block: u64,
    pub side: Side,
    pub token_in: Address,
    pub amount: U256,
}

/// Что делает: перемер перед паузой (см. шапку). Принимает: пулы, запись из памяти, что сверялось, итог первой
/// сверки. Возвращает: итог стороны — первый как есть (не пауза, нет крючка B/C, настройка не задана) или после
/// перемера. Ошибки: как у quoter_compare::compare (только от первой сверки — её итог возвращается как был).
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
    let tag = call::block_tag(t.block);
    let i = match hooks_probe::measure_on(&p.ctx, &p.tokens, &p.reg, t.fresh, &tag).await {
        Ok(Ok(i)) => i,
        Ok(Err(why)) => {
            return Ok(note(
                first,
                &format!(
                    "перемер влияния крючка на блоке {} не удался: {why}",
                    t.block
                ),
            ));
        }
        Err(err) => {
            return Ok(note(
                first,
                &format!(
                    "перемер влияния крючка на блоке {} не удался: {}",
                    t.block, err.message
                ),
            ));
        }
    };
    let before = p.checks.pool(&e.pool.reference).hook.map(|c| c.impact);
    let pc = hooks_probe::pool_check(&p.ctx, e, group, CheckState::Known(i.clone()));
    p.checks.update_pool(&e.pool.reference, |f| {
        f.hook = Some(pc.clone());
        f.hook_wait = None;
    });
    let _ = p.out.send(MarketEvent::PoolCheck(pc)).await;
    let pct = side_pct(&i, t.side);
    let second = quoter_compare::compare(&p.ctx, t.fresh, t.token_in, t.amount, pct, t.block).await;
    let res = merge(first, second, &i, t.side, t.block);
    let msg = format!(
        "{} {} ({}): перед паузой влияние крючка перемерено на блоке {}: было {}, стало покупка {}% / продажа {}% → {}",
        e.family.title,
        e.ticker,
        super::quoter_pause::side_name(t.side),
        t.block,
        was(&before),
        i.buy_pct.round_dp(4),
        i.sell_pct.round_dp(4),
        if matches!(res, Cmp::Pause { .. }) {
            "пауза"
        } else {
            "сверка без паузы"
        }
    );
    p.ctx.log(Level::Info, "dex.hook_reprobe", msg, None).await;
    Ok(res)
}

/// Влияние на сторону, %. Ошибок нет.
pub fn side_pct(i: &HookImpact, side: Side) -> Decimal {
    if side == Side::Buy {
        i.buy_pct
    } else {
        i.sell_pct
    }
}

/// Что делает: итог после перемера (чистая, тестируется на цифрах живого случая). Повторная сверка — пауза: пауза с
/// цифрами повторной сверки и пометкой перемера (и «отрицательное», если влияние стороны меньше 0); иная причина или
/// прошла — она; ошибка повторной сверки — первая пауза с пометкой. Принимает: первую паузу, итог повторной сверки,
/// новое влияние, сторону, блок. Возвращает: Cmp. Ошибок нет.
pub fn merge(
    first: Cmp,
    second: GatewayResult<Cmp>,
    i: &HookImpact,
    side: Side,
    block: u64,
) -> Cmp {
    match second {
        Ok(Cmp::Pause {
            block: b,
            why,
            nums,
        }) => {
            let pct = side_pct(i, side);
            let neg = if pct < Decimal::ZERO {
                format!(
                    "; {NEGATIVE} ({}%): квотер с крючком даёт больше нашей математики без крючка, вычитается 0 (quote/quoter.rs less_hook, решение 261)",
                    pct.round_dp(4)
                )
            } else {
                String::new()
            };
            Cmp::Pause {
                block: b,
                why: format!(
                    "{why} (после перемера влияния крючка на этом же блоке {block}: покупка {}%, продажа {}%{neg})",
                    i.buy_pct.round_dp(4),
                    i.sell_pct.round_dp(4)
                ),
                nums,
            }
        }
        Ok(other) => other,
        Err(err) => note(
            first,
            &format!(
                "повторная сверка после перемера не удалась: {}",
                err.message
            ),
        ),
    }
}

/// Пауза с дописанной причиной (иной итог — как есть). Ошибок нет.
fn note(c: Cmp, add: &str) -> Cmp {
    match c {
        Cmp::Pause { block, why, nums } => Cmp::Pause {
            block,
            why: format!("{why}; {add}"),
            nums,
        },
        other => other,
    }
}

/// Прежнее влияние для журнала. Ошибок нет.
fn was(c: &Option<CheckState<HookImpact>>) -> String {
    match c {
        Some(CheckState::Known(i)) => format!(
            "покупка {}% / продажа {}%",
            i.buy_pct.round_dp(4),
            i.sell_pct.round_dp(4)
        ),
        Some(CheckState::Failed { reason }) => format!("не измерено ({reason})"),
        _ => "не измерено".into(),
    }
}
