//! Файл: venues/bsc_dex/exec/dry_fund_holder.rs
//! Что делает: запасной источник монет для пополнения кошелька в симуляции (решение 302, задача t133), когда пула-
//! источника V2/V3 вне маршрута нет (exec/dry_fund.rs) — держатель-синглтон: Uniswap V4 PoolManager или PancakeSwap
//! Infinity Vault, у которых лежат токены ВСЕХ их пулов. Включается необязательной настройкой presell.fund_fallback
//! = singleton; не задана — этот файл ничего не делает (пополнение 1 в 1 как до решения). Перевод — тот же приём, что у
//! пула V2/V3: первым вызовом той же eth_simulateV1 transfer токена от имени держателя (validation = false — без
//! подписи; так же check/probe.rs:117 и exec/dry_sim.rs берут from = адрес контракта пула).
//! Почему держатель годится даже тогда, когда пул маршрута живёт в нём же (факт кода контрактов):
//! - Uniswap V4 PoolManager (github.com/Uniswap/v4-core, src/PoolManager.sol): обмен — только дельты
//!   (_accountPoolBalanceDelta), balanceOfSelf() читают лишь sync и _settle, а _settle берёт оплату как
//!   «баланс сейчас − баланс при sync» (transient CurrencyReserves) — перевод ДО sync в оплату не входит; Pool.swap
//!   (src/libraries/Pool.sol) считает по slot0 / liquidity / ticks, балансы токенов не читает; take выплачивает
//!   встречный токен — его баланс у держателя пополнение не трогает.
//! - PancakeSwap Infinity Vault (github.com/pancakeswap/infinity-core, src/Vault.sol): то же — balanceOfSelf() только в
//!   sync / _settle (VaultReserve), учёт пулов reservesOfApp меняется дельтами; CLPool.swap
//!   (src/pool-cl/libraries/CLPool.sol) и BinPool.swap (src/pool-bin/libraries/BinPool.sol) — по своему состоянию
//!   (slot0, liquidity, ticks / reserveOfBin), без чтения балансов.
//!
//! Значит изъятие монеты у держателя цену обмена не меняет; нехватка у держателя токена, который сделка забирает (take),
//! может дать только откат (отказ), но не завышенный выход. Честность НЕ гарантируется и держатель НЕ берётся, если в
//! маршруте есть пул в этом же держателе: (1) не V4 / Infinity CL / Infinity Bin (например Infinity Stable — StableSwap в
//! крючке, ссылка — адрес крючка, а токены — в Vault: pools/verify_inf_stable.rs, families.rs), (2) с крючком, вызываемым
//! при обмене (before/after swap, в т. ч. returns delta: код крючка мог бы читать баланс держателя) или с не прочитанными
//! крючками (hooks = None), (3) держатель пула маршрута не известен (Infinity / Balancer без dex.vault). Налог перевода и
//! излишек пары reflection-токена учитываются как у пула-источника (dry_fund::gross, exec/dry_surplus*.rs — замер
//! пар первого шага вокруг перевода от любого источника).
//! Кто вызывает: exec/dry_fund.rs (plan).
//! От чего зависит: ctx.rs, quote/mod.rs (Quoting), pools/{registry, families, depth_book}.rs, node/call.rs (view_many),
//! node/abi.rs (IERC20).
//! Настройки: presell.fund_fallback [bsc_dex]; dex.vault [DEX] (Vault Infinity / Balancer — держатель пула).

use alloy_primitives::{Address, U256};

use crate::core::types::{HookInfo, PoolRef};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::pools::depth_book;
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;
use super::super::quote::Quoting;

/// Имя настройки запасного источника (docs/SETTINGS.md).
pub const SETTING: &str = "presell.fund_fallback";
/// Вариант «держатель-синглтон» (единственный вариант настройки).
pub const SINGLETON: &str = "singleton";

/// Что выбор знает о пуле: кто физически держит его токены, считает ли вид обмен по своему состоянию (V4 / Infinity
/// CL / Bin), есть ли крючок обмена (None — крючки не прочитаны), подпись для журнала.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// None — держатель не известен (Infinity / Balancer без dex.vault).
    pub holder: Option<Address>,
    pub delta_math: bool,
    pub swap_hook: Option<bool>,
    pub label: String,
}

/// Значение настройки. Принимает: окружение. Возвращает: Some(вариант) или None — не задана.
/// Ошибки: Internal (хранилище, неизвестный вариант).
pub fn fallback(ctx: &Ctx) -> GatewayResult<Option<&'static str>> {
    match ctx.text(SETTING) {
        Ok(v) if v == SINGLETON => Ok(Some(SINGLETON)),
        Ok(v) => Err(ctx.err(
            ErrorCode::Internal,
            format!("{SETTING}: неизвестный вариант «{v}»"),
        )),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(e),
    }
}

/// Держатель токенов пула. Принимает: протокол DEX, ссылку пула, Vault DEX (dex.vault; None — не задан).
/// Возвращает: адрес держателя или None — не известен. Ошибок нет.
pub fn holder(proto: Proto, reference: &PoolRef, vault: Option<Address>) -> Option<Address> {
    match (reference, proto) {
        (PoolRef::Singleton { manager, .. }, Proto::UniswapV4) => Some(*manager),
        (PoolRef::Singleton { .. }, _) => vault,
        // Balancer V2 и Infinity Stable (ссылка — адрес крючка): токены пула лежат в Vault, а не у контракта по ссылке.
        (_, Proto::BalancerV2 | Proto::InfinityStable) => vault,
        (r, _) => Some(r.address()),
    }
}

/// Крючок вызывается при обмене (флаги before/after swap, в т. ч. returns delta). Принимает: крючки пула (None — не
/// прочитаны). Возвращает: Some(да/нет) или None — не известно. Ошибок нет.
pub fn swap_hook(h: Option<&HookInfo>) -> Option<bool> {
    h.map(|h| {
        !h.address.is_zero()
            && (h.before_swap
                || h.after_swap
                || h.before_swap_returns_delta
                || h.after_swap_returns_delta)
    })
}

/// Описание пула реестра для выбора. Принимает: окружение (dex.vault), пул. Возвращает: Site. Ошибки: Internal.
pub fn site(ctx: &Ctx, e: &Entry) -> GatewayResult<Site> {
    let vault = match ctx.dex_addr("dex.vault", e.family.id) {
        Ok(a) => Some(a),
        Err(x) if x.code == ErrorCode::NotConfigured => None,
        Err(x) => return Err(x),
    };
    Ok(Site {
        holder: holder(e.family.proto, &e.pool.reference, vault),
        delta_math: matches!(
            e.family.proto,
            Proto::UniswapV4 | Proto::InfinityCl | Proto::InfinityBin
        ),
        swap_hook: swap_hook(e.pool.hooks.as_ref()),
        label: format!(
            "{} {}",
            e.family.title,
            depth_book::label(&e.pool.reference)
        ),
    })
}

/// Годится ли держатель при этом маршруте. Принимает: держатель, пулы маршрута. Возвращает: Ok или Err(причина).
/// Ошибок нет.
pub fn route_ok(src: Address, route: &[Site]) -> Result<(), String> {
    for s in route {
        match s.holder {
            None => {
                return Err(format!(
                    "держатель токенов пула маршрута {} не известен (dex.vault не задан) — честность не доказать",
                    s.label
                ));
            }
            Some(h) if h != src => {}
            Some(_) if !s.delta_math => {
                return Err(format!(
                    "пул маршрута {} лежит в том же держателе {src:#x} и считает обмен не только по своему состоянию",
                    s.label
                ));
            }
            Some(_) if s.swap_hook == Some(true) => {
                return Err(format!(
                    "пул маршрута {} в том же держателе {src:#x} с крючком обмена — крючок мог бы читать баланс держателя",
                    s.label
                ));
            }
            Some(_) if s.swap_hook.is_none() => {
                return Err(format!(
                    "пул маршрута {} в том же держателе {src:#x}: крючки пула не прочитаны — честность не доказать",
                    s.label
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Выбор: среди держателей, годных при маршруте, — с наибольшим балансом ≥ суммы. Принимает: (держатель, баланс),
/// пулы маршрута, сумму. Возвращает: Ok(держатель) или Err(причина: кандидатов нет / не годятся / мало баланса).
/// Ошибок нет.
pub fn choose(cands: &[(Address, U256)], route: &[Site], need: U256) -> Result<Address, String> {
    if cands.is_empty() {
        return Err(
            "держателя-синглтона (Uniswap V4 PoolManager / Infinity Vault) с пулом этой монеты нет"
                .into(),
        );
    }
    let mut why = Vec::new();
    let mut best: Option<(Address, U256)> = None;
    for &(a, b) in cands {
        if let Err(w) = route_ok(a, route) {
            why.push(w);
        } else if b < need {
            why.push(format!("у держателя {a:#x} {b} единиц, нужно {need}"));
        } else if best.is_none_or(|(_, x)| b > x) {
            best = Some((a, b));
        }
    }
    best.map(|(a, _)| a).ok_or_else(|| why.join("; "))
}

/// Держатель-источник для монеты. Принимает: котировку (реестр, узел), токен, пулы маршрута, сумму перевода.
/// Возвращает: Ok(Ok((держатель, подпись для журнала))) или Ok(Err(причина)). Ошибки: узел (balanceOf), Internal.
pub async fn pick(
    q: &Quoting,
    token: Address,
    route: &[&Entry],
    need: U256,
) -> GatewayResult<Result<(Address, String), String>> {
    let ctx = &q.ctx;
    let mut holders: Vec<(Address, &'static str)> = Vec::new();
    // Снимок без копирования записей; только достоверные пулы (как source в exec/dry_fund.rs).
    for e in q.reg.snapshot() {
        let single = matches!(e.pool.reference, PoolRef::Singleton { .. });
        if !e.valid || !single || (e.pool.token0 != token && e.pool.token1 != token) {
            continue;
        }
        if let Some(h) = site(ctx, &e)?.holder
            && !holders.iter().any(|(a, _)| *a == h)
        {
            holders.push((h, e.family.title));
        }
    }
    let sites = route
        .iter()
        .map(|e| site(ctx, e))
        .collect::<GatewayResult<Vec<_>>>()?;
    let calls: Vec<_> = holders
        .iter()
        .map(|(a, _)| IERC20::balanceOfCall { owner: *a })
        .collect();
    let balances = call::view_many(ctx, token, &calls, "latest").await?;
    let cands: Vec<(Address, U256)> = holders.iter().map(|(a, _)| *a).zip(balances).collect();
    Ok(choose(&cands, &sites, need).map(|a| {
        let dex = holders
            .iter()
            .find(|(h, _)| *h == a)
            .map_or("", |(_, t)| *t);
        (
            a,
            format!("держателя-синглтона ({dex}; {SETTING} = {SINGLETON})"),
        )
    }))
}
