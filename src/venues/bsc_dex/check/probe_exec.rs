//! Файл: venues/bsc_dex/check/probe_exec.rs
//! Что делает: проба налога (check/probe.rs) через пул вида без своего пути исполнения (family.tradable = false: THENA
//! Solidly / Fusion, Topaz Slipstream, форки V3, DODO, StableSwap, Maverick), исполнимого шагом своего контракта-
//! исполнителя ArbExecutor (pools/exec_kind.rs::via_executor; решение 122, часть X). Своп пробы — тот же вызов, что у
//! настоящей сделки через исполнитель (exec/executor/trade.rs): `execute` развёрнутого исполнителя
//! (exec.executor_address; код и владелец сверены с узлом — exec/executor/onchain.rs) с одним шагом этого пула
//! (exec/executor/call.rs, вид шага — exec/executor/entry.rs) и обычное ERC-20 approve кошелька исполнителю
//! (wallet/approve_executor.rs) — в той же симуляции eth_simulateV1 от кошелька, код не подставляется.
//! Здесь же: причина «пробу собрать нечем» (вида шага нет — не настройка; шаг есть, адрес исполнителя не задан —
//! настройка exec.executor_address; обратного вызова пула dex.v3_swap_callback нет в коде исполнителя — не настройка,
//! ревью 281, pools/exec_callback.rs) и проверка «кто меняет» для обмена через исполнитель.
//! Задача t114: после сверки исполнителя (onchain::ensure пишет версию у кошелька) обратный вызов пула ещё раз
//! проверяется по сверенной версии (pools/exec_callback.rs) — нет: NoRoute, а не проба с откатом. Задача t122 (п. 1):
//! версия — прямо из результата onchain::ensure (exec/executor/callback_at.rs missing_at), не из записи у кошелька;
//! причина отказа — её текст (no_route).
//! Кто вызывает: check/probe.rs (part, emitter).
//! От чего зависит: exec/executor/{mod, call, callback_at, entry, onchain}.rs, exec/chain.rs (Step), exec/plan.rs (Order),
//! wallet/approve_executor.rs, pools/exec_kind.rs, pools/abi_balancer.rs (событие Swap хранилища Balancer, решение 289),
//! check/emitter.rs, check/simulate.rs (SimLog).
//! Настройки: exec.executor_address [bsc_dex]; dex.vault [DEX вида balancer_v2]; через вызываемые — dex.deadline_s,
//! dex.approve_unlimited, dex.v2_fee_pct [DEX].
//!
//! Как налог через исполнитель сопоставлен с настоящей сделкой — docs/notes/probe-executor-2026-11-03.md.
//! Кратко: покупка — выход шага получает кошелёк (у видов со своим выходом — прямо от пула, у StableSwap — через
//! исполнитель и его _sweep), «пришло» = прирост баланса кошелька, т. е. ровно то, что получит кошелёк в сделке;
//! продажа — налог «кошелёк → пул» прямым переводом, как раньше: через исполнитель монета с налогом продаётся только
//! первым шагом «вход переводом в пул» (V2 / SOLIDLY / DODO / WOO — transferFrom кошелёк → пул, один перевод;
//! quote/graph_executor.rs::tax), остальные виды с налогом на входе неисполнимы (сигнал), и это решает сама котировка
//! по измеренному налогу.

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::chain::Step;
use super::super::exec::executor::{self, call, callback_at, entry, onchain};
use super::super::exec::plan::Order;
use super::super::pools::abi_balancer::IBalancerVault;
use super::super::pools::abi_balancer_v3::IBalancerV3Vault;
use super::super::pools::registry::Entry;
use super::super::pools::{exec_callback, exec_kind};
use super::super::wallet::approve::Approval;
use super::super::wallet::approve_executor;
use super::emitter;
use super::simulate::SimLog;

/// Части свопа пробы через исполнитель: (разрешения кошелька исполнителю, адрес исполнителя, данные execute).
/// Принимает: пул (tradable = false), ордер пробы (минимум 1 единица), кошелёк, время блока. Один шаг на весь вход
/// (10000 bps) — как сделка одной долей через этот пул; минимум execute — минимум ордера.
/// Ошибки: NotConfigured — не задан exec.executor_address (у вида есть шаг) или настройки шага (dex.deadline_s,
/// dex.v2_fee_pct); NoRoute — у вида нет шага исполнителя (не настройка) или пул шагом не исполнить (entry.rs);
/// InvalidAddress — по адресу не наш исполнитель или владелец не кошелёк (onchain.rs); узел.
pub async fn part(
    ctx: &Ctx,
    e: &Entry,
    order: Order,
    wallet: Address,
    now: u64,
) -> GatewayResult<(Vec<Approval>, Address, Bytes)> {
    if !exec_kind::via_executor(ctx, e.family) {
        return Err(no_path(ctx, e));
    }
    let Some(x) = executor::address(ctx)? else {
        return Err(no_path(ctx, e));
    };
    // Ревью 122, пункт 1: адрес вписан руками — до разрешений код и владелец сверяются с узлом (успех помнится).
    let version = onchain::ensure(ctx, x, wallet).await?;
    // Задача t114: обратного вызова (или вида шага) нет в сверенной версии — не проба (NoRoute). Задача t122 (п. 1):
    // версия — из результата ensure, причина — её текст.
    if let Some(why) = callback_at::missing_at(ctx, e.family, version) {
        return Err(no_route(ctx, e, &why));
    }
    let step = Step {
        entry: e.clone(),
        order,
        from_wallet: true,
        to_wallet: true,
    };
    let c = call::build(ctx, &[vec![step]], x, now)?;
    // Симуляция не знает текущих разрешений — все сразу (как сухая сделка и проба UR: plan::build(…, true)).
    let approvals = approve_executor::all(ctx, c.token_in, x, c.amount_in)?;
    Ok((approvals, x, c.data()))
}

/// Причина «пробу через этот пул собрать нечем» (решение 122 F) — для журнала и экрана «Монеты». Шаг исполнителя у
/// вида есть, но у кошелька проб не развёрнут исполнитель (решение 132; NotConfigured — повтор кнопкой «Перепроверить»
/// или при следующем круге проверок); вида шага нет — не настройка (NoRoute: «не проверено» с причиной, без записи «не задана настройка»).
/// Принимает: окружение, пул. Возвращает: ошибку. Ошибок иных нет.
pub fn no_path(ctx: &Ctx, e: &Entry) -> GatewayError {
    let t = e.family.title;
    // Ревью 281 (В4): обратного вызова пула нет у исполнителя — не настройка и не вердикт по монете (NoRoute).
    if let Some(why) = e
        .family
        .executor_kind
        .and(exec_callback::missing(ctx, e.family))
    {
        return no_route(ctx, e, &why);
    }
    match e.family.executor_kind {
        Some(k) => ctx.err(
            ErrorCode::NotConfigured,
            format!(
                "{t}: проба налога через этот пул идёт шагом {} контракта-исполнителя, а у кошелька «{}» (строка \
                 «Пробы и сигналы», экран «Маппинг кошельков») не развёрнут исполнитель; налог — по другим пулам монеты",
                k.name(),
                ctx.wallet_name()
            ),
        ),
        None => ctx.err(
            ErrorCode::NoRoute,
            format!(
                "{t}: проба налога через этот пул невозможна — ни своего пути исполнения, ни шага исполнителя у \
                 вида нет; налог — по другим пулам монеты"
            ),
        ),
    }
}

/// Причина «у исполнителя нет пути для этого пула» (ревью 281 В4; задача t122 — общий текст для no_path и part).
/// Принимает: окружение, пул, причину. Возвращает: NoRoute (не настройка и не вердикт по монете). Ошибок иных нет.
fn no_route(ctx: &Ctx, e: &Entry, why: &str) -> GatewayError {
    let t = e.family.title;
    ctx.err(
        ErrorCode::NoRoute,
        format!("{t}: без пути исполнения: {why}; налог — по другим пулам монеты"),
    )
}

/// «Кто меняет» (решение 59) для обмена пробы. Пул со своим путём (Universal Router / роутер форка) — check/emitter.rs
/// как раньше: роутер сам ищет пару по фабрике. Через исполнитель пул вызывается ровно по адресу шага (как в сделке,
/// exec/executor/sim.rs) — подменить его некому; проверяется, что этот пул в обмене сам выпустил событие (у каждого
/// вида оно есть: Swap / Sync, DODOSwap, TokenExchange, TokenSwap, PoolSwap, WooSwap). Список топиков emitter.rs
/// (V2 / V3 / V4) здесь не годится: события DODO, StableSwap, Maverick в нём нет, а «обмен от другого пула» у монеты с
/// налогом (её обмен накопленного налога внутри перевода) исключил бы чужой по формату пул ложно.
/// Принимает: пул, события вызова обмена. Возвращает: Ok или ошибку с причиной: NotConfigured — пул со своим путём
/// исключён (как раньше, check/probe.rs); NoRoute — через исполнитель событие пула не найдено (не настройка и не
/// вердикт по монете: «не проверено», пул не исключается).
pub async fn emitter(ctx: &Ctx, e: &Entry, logs: &[SimLog]) -> GatewayResult<()> {
    if e.family.variant.umbrae(){
        let topic=super::super::pools::umbrae::swap_topic(e.family.variant);
        return logs.iter().any(|l|l.address==e.pool.reference.address() && l.topics.first()==Some(&topic)).then_some(()).ok_or_else(||ctx.err(ErrorCode::NoRoute,"У проверенной пары Umbrae нет события Swap"));
    }
    if super::super::pools::sectorone::enabled(e) {
        let swap=if e.family.proto==super::super::pools::families::Proto::LbV20 {
            super::super::pools::abi_lb20::ILBPairV20::Swap::SIGNATURE_HASH
        }else{super::super::pools::abi_lb::ILBPair::Swap::SIGNATURE_HASH};
        return logs.iter().any(|l| l.address==e.pool.reference.address() && l.topics.first()==Some(&swap))
            .then_some(()).ok_or_else(||ctx.err(ErrorCode::NoRoute,"Рынок SectorOne не выпустил событие Swap"));
    }
    if e.family.variant==super::super::pools::family_variant::Variant::VvsV3 {
        return logs.iter().any(|l|l.address==e.pool.reference.address() && l.topics.first()==Some(&super::super::pools::abi_v2v3::IPancakeV3Pool::Swap::SIGNATURE_HASH))
            .then_some(()).ok_or_else(||ctx.err(ErrorCode::NoRoute,"Пул VVS не выпустил зарегистрированное событие Swap"));
    }
    if e.family.variant==super::super::pools::family_variant::Variant::RegisteredUniV3 {
        return logs.iter().any(|l|l.address==e.pool.reference.address() && l.topics.first()==Some(&super::super::pools::abi_v2v3::IUniswapV3Pool::Swap::SIGNATURE_HASH))
            .then_some(()).ok_or_else(||ctx.err(ErrorCode::NoRoute,"Зарегистрированный V3 пул не выпустил событие Swap"));
    }
    if matches!(&e.pool.state,PoolState::Bundles(_) | PoolState::Dagrow(_) | PoolState::Antfarm(_) | PoolState::MintClub(_) | PoolState::Temple(_) | PoolState::Carbon(_)) || e.family.tradable
        || super::super::quote::frax_pricing::is_frax(e)
        || super::super::quote::oasis_pricing::is_oasis(e)
        || super::super::quote::vista_pricing::native_router(e)
    {
        return emitter::check(ctx, &[e], logs)
            .await
            .map_err(|why| ctx.err(ErrorCode::NotConfigured, why));
    }
    // Ревью 2 t181: та же проверка шага, что маршрут (entry::fits).
    let pool = entry::fits(ctx, e)
        .map_err(|why| ctx.err(ErrorCode::NoRoute, why))?
        .1;
    if logs.iter().any(|l| l.address == pool) {
        return Ok(());
    }
    // Решение 289: у Balancer V2 событие обмена выпускает хранилище — Vault.Swap с id пула в topic1 (IVault.sol L598-L604,
    // Swaps.sol L297); пул (onSwap — view) событий не шлёт.
    if let PoolState::VaultPool(d) = &e.pool.state {
        let vault = ctx.dex_addr("dex.vault", e.family.id)?;
        // Решение 441: у Balancer V3 событие Swap хранилища V3 — адрес пула в topic1 (IVaultEvents.sol L53-L61; pool_id
        // состояния V3 — адрес пула словом).
        let swap = if d.v3.is_some() {
            IBalancerV3Vault::Swap::SIGNATURE_HASH
        } else {
            IBalancerVault::Swap::SIGNATURE_HASH
        };
        if logs.iter().any(|l| {
            l.address == vault
                && l.topics.first() == Some(&swap)
                && l.topics.get(1) == Some(&d.pool_id)
        }) {
            return Ok(());
        }
    }
    let why = format!(
        "{}: обмен через исполнитель прошёл, но пул {pool:#x} не выпустил ни одного события — проба не засчитана",
        e.family.title
    );
    Err(ctx.err(ErrorCode::NoRoute, why))
}
