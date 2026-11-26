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
//! Решение 271 (дополнение): у ног форка одно ERC-20 approve на пару (токен входа, роутер форка) — на сумму входов её ног.
//! Решение 122 R3: нативная монета BNB — у каждой роутер-группы своя WRAP_ETH (value) / UNWRAP_WETH по флагам шагов
//! (exec/bundle_native.rs::group_wrap); нога форка с BNB не собирается.
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
use super::plan::Plan;
use super::ur;
use super::{bundle_native, fork_swap};

struct ForkLeg {
    router: Address,
    path: Vec<Address>,
    amount: U256,
    minimum: U256,
    module_call: Option<Bytes>,
}

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
    /// Однопуловый модуль. В calldata закреплены адрес пула, предел выплаты и срок.
    PoolSwap {
        router: Address,
        data: Bytes,
        nonce: u64,
    },
}

impl BundleTx {
    /// Nonce транзакции. Ошибок нет.
    pub fn nonce(&self) -> u64 {
        match self {
            BundleTx::Approve { nonce, .. }
            | BundleTx::Swap { nonce, .. }
            | BundleTx::PoolSwap { nonce, .. }
            | BundleTx::ForkSwap { nonce, .. } => *nonce,
        }
    }

    /// Это своп (true) или разрешение (false). Ошибок нет.
    pub fn is_swap(&self) -> bool {
        !matches!(self, BundleTx::Approve { .. })
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
    if !super::super::pools::nami::path_ok(legs.iter().flatten().map(|s| &s.entry)) {
        return Err(ctx.err(ErrorCode::NoRoute, "Бандл Nami содержит другие шаги"));
    }
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
    // Данные ног форка — проверки по ногам в прежнем порядке (обёртка, роутер, путь, вход, минимум).
    let mut forks = Vec::with_capacity(fork_legs.len());
    for &leg in &fork_legs {
        // R3 (решение 122): у роутера форка обёртки WBNB нет — нога форка с BNB не собирается (защитно).
        if bundle_native::group_wrap(ctx, &[leg])? != super::native::Wrap::None {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "нога форка с нативной монетой BNB — обёртки WBNB у роутера форка нет, бандл не собирается",
            ));
        }
        let (router, _) = fork_router(ctx, leg)?;
        let path = fork_swap::token_path(ctx, leg)?;
        let amount = fork_swap::amount_in(ctx, leg)?;
        let min = fork_swap::min_out(ctx, leg)?;
        let module_call = if leg.iter().any(|s| s.entry.family.direct_module()) {
            let [step] = leg else {
                return Err(ctx.err(
                    ErrorCode::NoRoute,
                    "Прямой модуль исполняет один конкретный рынок за вызов",
                ));
            };
            super::super::pools::direct_module::verify(ctx, step.entry.family, router, "latest")
                .await?;
            let pool = super::super::pools::direct_module::market(ctx, &step.entry)?;
            Some(super::super::pools::solidly_v3::execution_data(
                pool, path[0], path[1], amount, min, deadline,
            ))
        } else {
            None
        };
        forks.push(ForkLeg {
            router,
            path,
            amount,
            minimum: min,
            module_call,
        });
    }
    for (i, leg) in forks.iter().enumerate() {
        let (router, token_in) = (leg.router, leg.path[0]);
        // Решение 271 (дополнение): одно разрешение на пару (токен входа, роутер форка) в бандле — перед ПЕРВОЙ ногой
        // пары, на СУММУ входов всех её ног (ERC-20 approve задаёт allowance, а не прибавляет; каждая нога списывает свой
        // вход). Раньше вторая нога пары слала свой approve по allowance до бандла — лишняя транзакция. Одна нога на
        // пару — как раньше (сумма = её вход). Котировка считает так же (quote/approve_bundle.rs).
        let pair = |other: &ForkLeg| (other.router, other.path[0]) == (router, token_in);
        let approvals = if forks[..i].iter().any(pair) {
            Vec::new()
        } else {
            let total = forks[i..]
                .iter()
                .filter(|f| pair(f))
                .fold(U256::ZERO, |a, f| a.saturating_add(f.amount));
            if all_approvals {
                approve::fork_all(ctx, token_in, router, total)?
            } else {
                approve::fork_missing(ctx, token_in, wallet, router, total).await?
            }
        };
        for (to, data) in approvals {
            txs.push(BundleTx::Approve { to, data, nonce });
            nonce += 1;
        }
        txs.push(match &leg.module_call {
            Some(data) => BundleTx::PoolSwap {
                router,
                data: data.clone(),
                nonce,
            },
            None => BundleTx::ForkSwap {
                router,
                path: leg.path.clone(),
                amount_in: leg.amount,
                min_out: leg.minimum,
                to: wallet,
                deadline,
                nonce,
            },
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
/// (форк + не-форк или два форка) один вызов не исполнит. Ошибки: NoRoute (нет шагов, у шага нет своего пути
/// исполнения — tradable = нет, смешанная/межроутерная нога), NotConfigured (dex.router форка не задан).
fn fork_router(ctx: &Ctx, leg: &[Step]) -> GatewayResult<(Address, &'static Family)> {
    let first = leg
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "нога форка без шагов"))?;
    let family = first.entry.family;
    // Ревью 280: нога без своего пути исполнения (tradable = нет: v2_fork_executor, v3_fork — исполняются только шагом
    // исполнителя) в бандл попасть не должна; попала (исполнитель исчез между котировкой и сделкой) — отказ ДО чтения
    // dex.router: оставшийся от прежнего вида или ошибочный роутер мог бы исполнить своп чужой фабрикой.
    if let Some(s) = leg.iter().find(|s| !s.entry.family.tradable) {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            format!(
                "у DEX {} нет своего пути исполнения — нужен исполнитель у кошелька (exec.executor_address)",
                s.entry.family.title
            ),
        ));
    }
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
        let other = ctx.dex_addr("dex.universal_router", s.entry.family.id)?;
        if other != router {
            // Решение 220: для разбора — какие роутеры пересеклись (семейство и адрес) и кошелёк, которым шла сделка.
            return Err(ctx.err(
                ErrorCode::NoRoute,
                format!(
                    "нога маршрута пересекает Universal Router'ы ({} {router} → {} {other}; кошелёк «{}») — в v1 \
                     бандл такую ногу не собирает (нужна межроутерная передача промежуточного токена, отдельная фаза)",
                    family.id,
                    s.entry.family.id,
                    ctx.wallet_name()
                ),
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
            commands.push(chain::routed_command(ctx, s, fmt, wallet, (router, permit2)).await?);
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
    // R3 (решение 122): BNB — WRAP_ETH на сумму входов ног группы (value транзакции) или UNWRAP_WETH на кошелёк.
    let wrap = bundle_native::group_wrap(ctx, &g.legs)?;
    let plan = Plan::new(
        ctx,
        (approvals, router, wallet),
        out_token,
        commands,
        total_min,
        deadline,
    )?
    .with_native(wrap);
    Ok((plan, legs_min))
}
