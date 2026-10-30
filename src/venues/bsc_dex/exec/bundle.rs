//! Файл: venues/bsc_dex/exec/bundle.rs
//! Хардинг ревью #79: минимум не 0 у КАЖДОГО шага ноги (L2, как exec/plan.rs), и явная проверка, что ноги одной
//! роутер-группы имеют один входной токен и одну экосистему (L3).
//! Что делает: сборка бандла свопов для мульти-роутерного графового маршрута (решение 84, фаза 80.8a). Маршрут,
//! выходящий за один Universal Router, исполняется НЕ одной транзакцией, а набором: по одному execute на роутер
//! (DEX-группу), атомарно в одном блоке через приватный релей (отправка — 80.8b). Ноги группируются по адресу
//! Universal Router; на каждую группу — один execute со всеми её цепочками команд (как умный путь, exec/universal.rs,
//! но для любого DEX, не только PancakeSwap). Каждая нога несёт свой minOut на уровне своп-команды (amountOutMinimum
//! пула — откат ноги при недоборе роняет весь бандл, решение 40); minOut берётся из вердикта verify (решение 83/84,
//! проставляется вызывающим в последний шаг ноги). Разрешения (ERC-20 → Permit2 → UR) — wallet/approve, отдельными
//! транзакциями перед свопами. Nonce строго по очереди (решение 45): разрешения, затем свопы, в порядке групп.
//! ЭТОТ модуль только СОБИРАЕТ бандл (без сети): цена и лимит газа, база баланса для BALANCE_CHECK и подпись —
//! при отправке (80.8b, переиспользуя send/tx.rs, wallet/signer.rs). Нога, пересекающая роутеры (нужна межроутерная
//! передача промежуточного токена), в v1 не собирается — отказ (позже).
//! Нога ФОРКА (решения 59, 82, фаза 80.9b) исполняется своим classic V2 Router, а не Universal Router: отдельной
//! транзакцией BundleTx::ForkSwap (exec/fork_swap.rs) с ERC-20 approve роутеру (wallet/approve::fork_*). В одном
//! атомарном бандле можно смешивать ноги UR (Pancake) и форка (Biswap) — цель решения 80.
//! Кто вызывает: exec/trade.rs (80.8b, для исполнимого бандлом графа); тесты.
//! От чего зависит: exec/chain.rs (Step, command), exec/plan.rs (Plan), exec/ur.rs (Format), wallet/approve.rs,
//! pools/families.rs (Family), pools/registry.rs.
//! Настройки: dex.universal_router, dex.permit2, dex.universal_router_version [DEX], dex.deadline_s,
//! dex.approve_unlimited, dex.permit2_expiration_s [bsc_dex] — через plan/ur/approve.

use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::families::Family;
use super::super::wallet::approve;
use super::chain::{self, Step};
use super::fork_swap;
use super::plan::Plan;
use super::ur;

/// Одна транзакция бандла с присвоенным nonce.
#[derive(Debug, Clone)]
pub enum BundleTx {
    /// Разрешение (ERC-20 approve или Permit2.approve): конкретные адрес и данные (готовы к подписи).
    Approve {
        to: Address,
        data: Bytes,
        nonce: u64,
    },
    /// Своп одного роутера (один execute Universal Router): план (данные достраиваются при отправке с базой баланса,
    /// Plan::data) и минимумы выхода по ногам этого роутера (per-leg, из вердикта verify) — для показа и проверки.
    Swap {
        plan: Plan,
        nonce: u64,
        legs_min: Vec<U256>,
    },
    /// Своп через собственный роутер форка (classic V2 Router, решения 59, 82, фаза 80.9b):
    /// swapExactTokensForTokensSupportingFeeOnTransferTokens(amount_in, min_out, path, to, deadline). Данные собираются
    /// при симуляции/подписи (exec/fork_swap::swap_data) — без базы BALANCE_CHECK: защита — amountOutMin самого роутера
    /// (решение 40, дельта баланса получателя). Разрешение — отдельной BundleTx::Approve (ERC-20 approve роутеру).
    ForkSwap {
        router: Address,
        path: Vec<Address>,
        amount_in: U256,
        min_out: U256,
        to: Address,
        deadline: U256,
        nonce: u64,
    },
}

impl BundleTx {
    /// Nonce транзакции. Ошибок нет.
    pub fn nonce(&self) -> u64 {
        match self {
            BundleTx::Approve { nonce, .. }
            | BundleTx::Swap { nonce, .. }
            | BundleTx::ForkSwap { nonce, .. } => *nonce,
        }
    }

    /// Это своп (true) или разрешение (false). Ошибок нет.
    pub fn is_swap(&self) -> bool {
        matches!(self, BundleTx::Swap { .. } | BundleTx::ForkSwap { .. })
    }
}

/// Бандл свопов мульти-роутерного маршрута: упорядоченные транзакции (разрешения, затем свопы — по одному на роутер),
/// nonce строго по очереди от first_nonce (решение 45). Готов к отправке eth_sendBundle (фаза 80.8b).
#[derive(Debug, Clone)]
pub struct Bundle {
    pub txs: Vec<BundleTx>,
    /// Первый nonce; следующие — по порядку.
    pub first_nonce: u64,
    /// Сколько свопов (роутеров/DEX-групп) в бандле.
    pub swaps: usize,
}

/// Группа ног одного Universal Router.
struct Group<'a> {
    router: Address,
    family: &'static Family,
    legs: Vec<&'a [Step]>,
}

/// Собирает бандл. Принимает: ноги в порядке исполнения (каждая — шаги exec/chain.rs с проставленным minOut верификации
/// в последнем шаге), выходной токен сделки (монета при покупке, USDT при продаже), кошелёк, первый nonce, время блока
/// (секунды), нужны ли все разрешения (true — не ходить на узел: для сборки/тестов; false — только недостающие, узел).
/// Возвращает: бандл. Ошибки: NoRoute (нет ног, нога без шагов, нога пересекает роутеры), QuoteMismatch (минимум 0),
/// NotConfigured (адреса/версия/срок), Internal (план), узел (только при all_approvals = false).
pub async fn build(
    ctx: &Ctx,
    legs: &[Vec<Step>],
    out_token: Address,
    wallet: Address,
    first_nonce: u64,
    now: u64,
    all_approvals: bool,
) -> GatewayResult<Bundle> {
    if legs.is_empty() {
        return Err(ctx.err(ErrorCode::NoRoute, "бандл без ног"));
    }
    // Ноги форка (исполняются своим роутером) — отдельно от ног Universal Router (group по UR). Нога форка = все её
    // шаги у форка; смешанную (форк + не-форк в одной ноге) один вызов не исполнит — fork_router это отвергает.
    let (fork_legs, ur_legs): (Vec<&[Step]>, Vec<&[Step]>) = legs
        .iter()
        .map(|l| l.as_slice())
        .partition(|l| l.first().is_some_and(|s| s.entry.family.fork));
    // Группы по адресу Universal Router, в порядке первого появления.
    let mut groups: Vec<Group<'_>> = Vec::new();
    for &leg in &ur_legs {
        let (router, family) = single_router(ctx, leg)?;
        match groups.iter_mut().find(|g| g.router == router) {
            Some(g) => g.legs.push(leg),
            None => groups.push(Group {
                router,
                family,
                legs: vec![leg],
            }),
        }
    }
    // Своп на группу UR + своп на ногу форка (у форка на ногу — один вызов роутера, execute-склейки нет).
    let swaps = groups.len() + fork_legs.len();
    let mut txs = Vec::new();
    let mut nonce = first_nonce;
    for g in &groups {
        let (mut plan, legs_min) =
            router_execute(ctx, g, out_token, wallet, now, all_approvals).await?;
        // Разрешения — отдельными транзакциями перед свопом роутера (вынимаем из плана: в бандле шлём их сами).
        for (to, data) in std::mem::take(&mut plan.approvals) {
            txs.push(BundleTx::Approve { to, data, nonce });
            nonce += 1;
        }
        txs.push(BundleTx::Swap {
            plan,
            nonce,
            legs_min,
        });
        nonce += 1;
    }
    // Ноги форка: ERC-20 approve роутеру форка (без Permit2) + своп его роутером. Кумулятивная база — через to=кошелёк
    // (роутер FeeOnTransfer меряет дельту поверх прошлых групп); выход форка попадает в базу последующих UR-групп, т.к.
    // их balanceOf в симуляции читается после этой транзакции (exec/bundle_sim.rs).
    let deadline = U256::from(now + ctx.int("dex.deadline_s")?);
    for &leg in &fork_legs {
        let (router, _family) = fork_router(ctx, leg)?;
        let path = fork_swap::token_path(ctx, leg)?;
        let amount = fork_swap::amount_in(ctx, leg)?;
        let min = fork_swap::min_out(ctx, leg)?;
        let token_in = path[0];
        let approvals = if all_approvals {
            approve::fork_all(ctx, token_in, router, amount)?
        } else {
            approve::fork_missing(ctx, token_in, wallet, router, amount).await?
        };
        for (to, data) in approvals {
            txs.push(BundleTx::Approve { to, data, nonce });
            nonce += 1;
        }
        txs.push(BundleTx::ForkSwap {
            router,
            path,
            amount_in: amount,
            min_out: min,
            to: wallet,
            deadline,
            nonce,
        });
        nonce += 1;
    }
    Ok(Bundle {
        txs,
        first_nonce,
        swaps,
    })
}

/// Адрес роутера форка ноги и её семейство. Все шаги — один форк и один его роутер (dex.router): межроутерную ногу
/// (форк + не-форк или два форка) один вызов не исполнит. Ошибки: NoRoute (нет шагов, смешанная/межроутерная нога),
/// NotConfigured (dex.router форка не задан).
fn fork_router(ctx: &Ctx, leg: &[Step]) -> GatewayResult<(Address, &'static Family)> {
    let first = leg
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "нога форка без шагов"))?;
    let family = first.entry.family;
    let router = ctx.dex_addr("dex.router", family.id)?;
    for s in leg {
        if !s.entry.family.fork {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "нога смешивает форк и не-форк — бандл такую ногу одним вызовом не собирает",
            ));
        }
        if ctx.dex_addr("dex.router", s.entry.family.id)? != router {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "нога форка пересекает роутеры — один вызов swapExactTokens… её не исполнит",
            ));
        }
    }
    Ok((router, family))
}

/// Адрес Universal Router ноги и её представительное семейство. Все шаги ноги должны идти через один роутер.
/// Ошибки: NoRoute (нога без шагов или пересекает роутеры — межроутерная передача в v1 не собирается), NotConfigured.
fn single_router(ctx: &Ctx, leg: &[Step]) -> GatewayResult<(Address, &'static Family)> {
    let first = leg
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "нога бандла без шагов"))?;
    let family = first.entry.family;
    let router = ctx.dex_addr("dex.universal_router", family.id)?;
    for s in leg {
        if ctx.dex_addr("dex.universal_router", s.entry.family.id)? != router {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "нога маршрута пересекает Universal Router'ы — в v1 бандл такую ногу не собирает \
                 (нужна межроутерная передача промежуточного токена, отдельная фаза)",
            ));
        }
    }
    Ok((router, family))
}

/// Один execute Universal Router на группу ног (все цепочки команд группы, минимум у каждого шага; BALANCE_CHECK на
/// выходной токен — сумма минимумов ног группы; решение 40). Возвращает: план и minOut каждой ноги группы. Ошибки:
/// QuoteMismatch (минимум ноги 0), NotConfigured (адреса, версия, срок), Internal (план), узел (all_approvals = false).
async fn router_execute(
    ctx: &Ctx,
    g: &Group<'_>,
    out_token: Address,
    wallet: Address,
    now: u64,
    all_approvals: bool,
) -> GatewayResult<(Plan, Vec<U256>)> {
    let router = ctx.dex_addr("dex.universal_router", g.family.id)?;
    let permit2 = ctx.dex_addr("dex.permit2", g.family.id)?;
    let deadline = U256::from(now + ctx.int("dex.deadline_s")?);
    // Формат входов — по первому шагу группы; у всех ног одного роутера он один (одна экосистема).
    let fmt = ur::format(ctx, &g.legs[0][0].entry)?;
    let tin = g.legs[0][0].order.token_in;
    let mut commands = Vec::new();
    let (mut total_in, mut total_min) = (U256::ZERO, U256::ZERO);
    let mut legs_min = Vec::with_capacity(g.legs.len());
    for leg in &g.legs {
        let (Some(first), Some(last)) = (leg.first(), leg.last()) else {
            return Err(ctx.err(ErrorCode::NoRoute, "нога бандла без шагов"));
        };
        // L3 (ревью #79): все ноги группы — один входной токен и одна экосистема (формат входов). Группировка по
        // адресу роутера это обычно обеспечивает, но берём fmt/разрешения по первой ноге — проверяем явно.
        if first.order.token_in != tin {
            return Err(ctx.err(
                ErrorCode::Internal,
                "ноги одной роутер-группы с разным входным токеном — разрешения и вход бандла не свести",
            ));
        }
        if ur::format(ctx, &first.entry)? != fmt {
            return Err(ctx.err(
                ErrorCode::Internal,
                "ноги одной роутер-группы из разных экосистем (формат Universal Router не совпал)",
            ));
        }
        if first.from_wallet {
            total_in = total_in.saturating_add(first.order.amount_in);
        }
        for s in *leg {
            // L2 (ревью #79): минимум не 0 у КАЖДОГО шага (как exec/plan.rs::build), не только у последнего —
            // промежуточный хоп не должен уйти с amountOutMinimum = 0.
            if s.order.min_out.is_zero() {
                return Err(ctx.err(
                    ErrorCode::QuoteMismatch,
                    "минимум выхода шага 0 — обмен без минимума в бандл не ставится",
                ));
            }
            commands.push(chain::command(ctx, s, fmt, wallet)?);
        }
        total_min = total_min.saturating_add(last.order.min_out);
        legs_min.push(last.order.min_out);
    }
    // Разрешения на входной токен (USDT при покупке, монета при продаже) — Permit2 → этот роутер, на сумму входов группы.
    let approvals = if total_in.is_zero() {
        Vec::new()
    } else if all_approvals {
        approve::all(ctx, tin, permit2, router, total_in, now)?
    } else {
        approve::missing(ctx, tin, wallet, permit2, router, total_in, now).await?
    };
    let plan = Plan::new(
        ctx,
        (approvals, router, wallet),
        out_token,
        commands,
        total_min,
        deadline,
    )?;
    Ok((plan, legs_min))
}
