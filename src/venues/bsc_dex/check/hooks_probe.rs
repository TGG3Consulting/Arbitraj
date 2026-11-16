//! Файл: venues/bsc_dex/check/hooks_probe.rs
//! Что делает: влияние крючка пула V4 группы B/C (решение 32) — пробный обмен квотером своей DEX (он исполняет
//! крючки) против локальной математики пула без крючка, на сумме check.probe_usdt: покупка (USDT → монета)
//! и продажа (монеты на эту сумму → USDT). Пул с промежуточной монетой (решение 52) — проба в ней (quote/rate.rs).
//! Влияние, % = (математика − квотер) / математика · 100; положительное — получаем меньше. Ошибка пробы (откат
//! квотера, крючок требует hookData и т.п.) — Ok(Err) с причиной; откат квотера NotEnoughLiquidity(PoolId) — причина
//! «в пуле не хватает ликвидности для пробы check.probe_usdt (… PoolId …)», не «проверка крючка не прошла».
//! Решение «335 (В1)»: влияние меряется ТОЛЬКО на одном блоке — состояние пула на блоке H и квотер eth_call на тот же
//! H. Путь «память бота против квотера на latest» убран: разница памяти и сети попадала во «влияние крючка» (записи
//! узла 16.11: на одном блоке влияние у 28 пулов с паузой — 0, живое расхождение — сохранённый ложный процент).
//! - measure_on: состояние уже прочитано на H (цикл крючков check/hooks_head.rs, перемер перед паузой
//!   check/quoter_reprobe.rs). Два вызова квотера (покупка и продажа) — параллельно: сумма продажи считается по
//!   прочитанному состоянию без узла.
//! - measure_read: перед сделкой (exec/precheck.rs hook_again) — суммы по памяти (только суммы, не расчёт), блок H —
//!   голова из node/heads.rs (без eth_blockNumber); чтение пула на H (pools/load_batch.rs, Multicall3-ступени, как у
//!   предторговой верификации, решение 83) и оба вызова квотера на H — ОДНОВРЕМЕННО; математика — по прочитанному.
//!
//! Кто вызывает: check/hooks_head.rs, check/quoter_reprobe.rs, exec/precheck.rs.
//! От чего зависит: quote/quoter.rs, quote/pool_quote.rs, quote/rate.rs, pools/load_batch.rs, pools/load.rs,
//! pools/registry.rs, tokens.rs, num.rs, node/call.rs.
//! Настройки: check.probe_usdt [bsc_dex]; dex.quoter [вид пула]; через load_batch — настройки чтения пула.
//!
//! Квотер выполняет настоящий swap внутри vault.lock и откатывается — IP/src/pool-cl/lens/CLQuoter.sol#L27-L38
//! (research/flow/bsc_v4.md, раздел 5): «hook-и пула в квоте исполняются».

use alloy_primitives::{Address, U256};
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, CheckState, HookGroup, HookImpact, PoolCheck};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::num;
use super::super::pools::load;
use super::super::pools::load_batch::{self, Want};
use super::super::pools::registry::{Entry, Registry};
use super::super::quote::{pool_quote, quoter, quoter_revert, rate};
use super::super::tokens::Tokens;

/// Итог пробы: влияние или причина, почему не посчитать.
pub type Measured = GatewayResult<Result<HookImpact, String>>;

/// Пара сумм или выходов пробы: (покупка, продажа).
type Pair = (U256, U256);

/// Итог проверки крючка пула для ядра и экрана (вынесено решением 335 — тот же текст полей для пробы на блоке H).
/// Принимает: окружение, запись, группу крючка, итог влияния. Возвращает: PoolCheck со временем «сейчас». Ошибок нет.
pub fn pool_check(
    ctx: &Ctx,
    e: &Entry,
    group: HookGroup,
    impact: CheckState<HookImpact>,
) -> PoolCheck {
    PoolCheck {
        venue: ctx.venue(),
        family: e.family.id.to_string(),
        pool: e.pool.reference.clone(),
        asset: Asset {
            ticker: e.ticker.clone(),
            bsc_contract: Some(e.coin),
        },
        group,
        impact,
        checked_at: Some(Utc::now()),
    }
}

/// Начало причины «в пуле не хватает ликвидности для пробы» — не провал крючка, а свойство пула на сумму пробы:
/// так причина и показывается (check/quoter_verify.rs, quote/select.rs, check/cycle.rs — без «проверка крючка не
/// прошла»).
pub const SHORT: &str = "в пуле не хватает ликвидности для пробы check.probe_usdt";

/// Причина по откату квотера NotEnoughLiquidity(PoolId). Ошибок нет.
fn short(id: alloy_primitives::B256) -> String {
    format!("{SHORT} (NotEnoughLiquidity, PoolId {id})")
}

/// Квотер на пробу на блоке `tag`; откат «не хватает ликвидности» (UnexpectedRevertBytes(NotEnoughLiquidity(PoolId)),
/// quote/quoter_revert.rs) — Ok(Err(причина)). Ошибки: как у quoter::out_at.
async fn ask(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
    tag: &str,
) -> GatewayResult<Result<U256, String>> {
    match quoter::out_at(ctx, e, token_in, amount, tag).await {
        Ok(v) => Ok(Ok(v)),
        Err(err) => match quoter_revert::short_pool(&err) {
            Some(id) => Ok(Err(short(id))),
            None => Err(err),
        },
    }
}

/// Суммы пробы по записи `e`: (вход покупки во встречном токене — check.probe_usdt, через промежуточную монету по
/// решению 52; монеты продажи — выход нашей покупки на эту сумму). Ok(Err) — посчитать нельзя (причина).
/// Ошибки: настройки, узел (decimals), математика пула.
async fn amounts(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    e: &Entry,
) -> GatewayResult<Result<Pair, String>> {
    let usdt = tokens.usdt(ctx)?;
    let usdt_dec = tokens.decimals(ctx, usdt).await?;
    let Some(p) = num::to_units(ctx.dec("check.probe_usdt")?, usdt_dec) else {
        return Ok(Err("check.probe_usdt не переводится в единицы USDT".into()));
    };
    let p = match rate::from_usdt(ctx, reg, usdt, e.usdt, p) {
        Ok(a) => a,
        Err(err) => return Ok(Err(err.message)),
    };
    let coins = pool_quote::swap(ctx, e, e.usdt, true, p)?.amount_out;
    Ok(Ok((p, coins)))
}

/// Оба вызова квотера на блоке `tag` одновременно (покупка, продажа). Ошибки: как у ask.
async fn asks(
    ctx: &Ctx,
    e: &Entry,
    (p, coins): Pair,
    tag: &str,
) -> GatewayResult<Result<Pair, String>> {
    let (buy, sell) = tokio::join!(ask(ctx, e, e.usdt, p, tag), ask(ctx, e, e.coin, coins, tag));
    match (buy?, sell?) {
        (Ok(b), Ok(s)) => Ok(Ok((b, s))),
        (Err(why), _) | (_, Err(why)) => Ok(Err(why)),
    }
}

/// Влияние по состоянию `fresh` (на блоке квотера) на тех же суммах. Ошибки: математика пула.
fn judge(ctx: &Ctx, fresh: &Entry, (p, coins): Pair, (rb, rs): Pair) -> Measured {
    let lb = pool_quote::swap(ctx, fresh, fresh.usdt, true, p)?;
    let ls = pool_quote::swap(ctx, fresh, fresh.coin, true, coins)?;
    match (impact(lb.amount_out, rb), impact(ls.amount_out, rs)) {
        (Some(buy_pct), Some(sell_pct)) => Ok(Ok(HookImpact { buy_pct, sell_pct })),
        _ => Ok(Err("математика пула дала 0 — влияние не определить".into())),
    }
}

/// Покупка и продажа: математика по состоянию записи `e`, уже прочитанному на блоке `tag` (check/quoter_block.rs
/// at_head), против квотера на том же блоке. Принимает: окружение, токены, реестр (пересчёт суммы через
/// промежуточную монету), запись на блоке, блок (node/call.rs block_tag). Возвращает: Ok(Ok(влияние)) или
/// Ok(Err(причина)). Ошибки: настройки, узел (в том числе StaleData — нет состояния блока), откат квотера (кроме
/// «не хватает ликвидности»).
pub async fn measure_on(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    e: &Entry,
    tag: &str,
) -> Measured {
    let a = match amounts(ctx, tokens, reg, e).await? {
        Ok(a) => a,
        Err(why) => return Ok(Err(why)),
    };
    let r = match asks(ctx, e, a, tag).await? {
        Ok(r) => r,
        Err(why) => return Ok(Err(why)),
    };
    judge(ctx, e, a, r)
}

/// Перед сделкой: суммы — по памяти `e` (только суммы); пул читается на блоке `block` и оба вызова квотера на нём —
/// одновременно; математика — по прочитанному. Принимает: окружение, токены, реестр, запись из памяти, блок.
/// Возвращает: как measure_on. Ошибки: как measure_on и чтение пула (узел, не задана настройка чтения).
pub async fn measure_read(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    e: &Entry,
    block: u64,
) -> Measured {
    let a = match amounts(ctx, tokens, reg, e).await? {
        Ok(a) => a,
        Err(why) => return Ok(Err(why)),
    };
    let tag = call::block_tag(block);
    let want = [Want {
        family: e.family,
        reference: &e.pool.reference,
        spacing: e.pool.tick_spacing,
        step: e.pool.bin_step,
        usdt: e.usdt,
        token0: e.pool.token0,
        token1: e.pool.token1,
    }];
    let (read, r) = tokio::join!(
        load_batch::states(ctx, tokens, reg, &want, block),
        asks(ctx, e, a, &tag)
    );
    let loaded = read.into_iter().next().ok_or_else(|| {
        ctx.err(
            crate::gateway::errors::ErrorCode::BadResponse,
            "пул не прочитан",
        )
    })??;
    let r = match r? {
        Ok(r) => r,
        Err(why) => return Ok(Err(why)),
    };
    judge(ctx, &load::renewed(e, block, loaded), a, r)
}

/// (математика − квотер) / математика · 100 (тесты на записях узла — та же формула). None — математика 0 или числа
/// не помещаются в Decimal.
pub fn impact(local: U256, remote: U256) -> Option<Decimal> {
    let l = num::to_decimal(local, 0)?;
    let r = num::to_decimal(remote, 0)?;
    if l.is_zero() {
        return None;
    }
    Some((l - r) / l * Decimal::ONE_HUNDRED)
}
