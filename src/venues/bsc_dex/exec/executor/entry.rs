//! Файл: venues/bsc_dex/exec/executor/entry.rs
//! Что делает: шаг исполнителя (kind.rs Params) для пула реестра (pools/registry.rs Entry). Вид шага — пометка части F на
//! семействе (Family.executor_kind, pools/exec_kind.rs): V2 → V2 с комиссией пары (своя у пары или dex.v2_fee_pct DEX —
//! как котировка, quote/pool_quote.rs), V3 (PancakeSwap/Uniswap V3, форки, Slipstream) → V3, ALGEBRA, SOLIDLY, DODO.
//! Нет вида шага — пул исполнителем не вызывается (Infinity / V4 — только Universal Router; KyberSwap Elastic — вид
//! ELASTIC в контракте есть, но семейство неисполнимо, решение 121 B2). CURVE / WOO / MAVERICK из реестра пока не
//! приходят (семейств нет). CURVE (StableSwap Ellipsis, решение 122 F2) — индексы пары из состояния пула
//! (PoolState::StableSwap: i0 — token0, i1 — token1), направление — по токену входа шага; так же STABLE_U256
//! (PancakeSwap Stable, Lista) и SADDLE (Nerve) — решение 122 X, стейблсвапы; пул с halted — не исполняется.
//! Монета пары — нативный BNB пула (StableData.native, в паре — обёртка WBNB; решение «122 X (нативный BNB в
//! стейблсвапах)»): вид STABLE_U256_BNB (bnb_in — по тому, вход это или выход); у SADDLE — отказ; у CURVE (решение
//! 472, T472-08: пулы Curve с нативным ETH — Plain*ETH / Plain2ETHEMA) — шаг CURVE_ETH = 28 у исполнителя кошелька v5
//! (exec_callback.rs since), у v1–v4 и у несверенного — отказ с причиной («нужна сборка v5»).
//! Решение 273: пул Solidly с фабрикой на паузе — шаг не строится (NoRoute с причиной).
//! Решение 470 (T470-06): пул Aero Basic у исполнителя v5 — шаг AERO_BASIC (pools/exec_kind.rs for_wallet).
//! Решение 484: пул Wombat — шаг WOMBAT = 30 (только v5; пауза пула и актива входа — шаг не строится).
//! Решение 442: crypto-пул Curve (Twocrypto-NG / Tricrypto-NG, PoolState::Crypto) — шаг STABLE_U256 (exchange(uint256 i,
//! uint256 j, dx, 0)), индексы — из состояния пула; пул с halted (tricrypto 2021 is_killed) — не исполняется. Решение
//! 453: пул в рампе A/gamma исполняется — котировка считает рампу, как контракт (D пересчитывается через newton_D,
//! math/crypto_swap.rs), пул перечитывается каждым блоком, пока рампа идёт (pools/per_block_crypto.rs).
//! Решение 395 (t181): пул внутри менеджера (Uniswap V4 — PoolRef::Singleton PoolManager; Infinity CL / Bin — Singleton
//! своего менеджера; Infinity Stable — Contract(крючок)) — шаги UNI_V4 / INF_CL / INF_BIN исполнителя v3: ключ пула из
//! реестра (Entry.key) сверяется с ссылкой (PoolId ключа = id ссылки и менеджер ключа = менеджер ссылки; у Infinity
//! Stable — крючок ключа = адрес пула), пул шага — PoolManager (V4) или Vault Infinity (dex.vault).
//! Решение 400 (t185): шаг пула менеджера строится и признаётся исполнимым (fits, singleton), только если его класс (вид
//! шага × с крючком / без; Infinity Stable) принят записью симуляции на живых пулах — exec/executor/live_proof.rs.
//! Решение 401 (t186): правило — exec/executor/pool_gate.rs: к приёмке класса добавлен факт пробы ЭТОГО пула через
//! исполнитель кошелька окружения (прошёл — исполним и в непринятом классе; отвергнут — не исполним и в принятом).
//! Форма шага без правила (shape, singleton_step) — для самой пробы (check/pass_probe.rs, check/pass_cycle.rs).
//! Кто вызывает: exec/executor/{call, mod}.rs, quote/graph_exec.rs, check/{pass_probe, pass_cycle}.rs.
//! От чего зависит: exec/executor/{kind, live_proof, pool_gate}.rs, pools/{registry, families, exec_kind}.rs,
//! quote/pool_quote.rs (v2_fee_pct).
//! Настройки: через pool_quote — dex.v2_fee_pct [DEX] (если у пары нет своей комиссии); dex.vault [DEX вида
//! pancakeswap_infinity_* / pancake_infinity_stable] — для шагов INF_CL / INF_BIN.

use alloy_primitives::Address;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::types::{PoolRef, PoolState, ProteusData, SmartColDebtData, StableData};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::ctx::Ctx;
use super::super::super::pools::exec_callback;
use super::super::super::pools::exec_kind::{self, ExecKind};
use super::super::super::pools::families::{Family, Proto};
use super::super::super::pools::key::PoolKey;
use super::super::super::pools::registry::Entry;
use super::super::super::pools::stable_native;
use super::super::super::quote::pool_quote;
use super::code::Version;
use super::kind::{FluidStep, InfKey, Params, STABLE_NATIVE, ShellStep, V4Key};

/// Может ли шаг через этот пул идти исполнителем: у семейства есть вид шага (Family.executor_kind) и у пула свой
/// контракт. Принимает: семейство и ссылку на пул (хватает хопа маршрута). Возвращает: вид шага и адрес пула или
/// причину. Ошибок иных нет. Ревью t181 (Н2): у шагов пулов менеджеров (UNI_V4 / INF_*) ссылка — только Singleton;
/// Contract — только у Infinity Stable (пул в крючке пула CL, шаг INF_CL): исполнитель никогда не вызовет крючок или
/// другой контракт как менеджер. Ключ пула (нативная валюта, валюты пары) — fits (нужна запись реестра).
pub fn supported(f: &Family, pool: &PoolRef) -> Result<(ExecKind, Address), String> {
    let k = f
        .executor_kind
        .ok_or_else(|| format!("{}: у вида пула нет шага исполнителя", f.id))?;
    if k.singleton() {
        return match pool {
            PoolRef::Singleton { manager, .. } => Ok((k, *manager)),
            PoolRef::Contract(hook) if k == ExecKind::InfCl && f.proto == Proto::InfinityStable => {
                Ok((k, *hook))
            }
            other => Err(format!(
                "{}: ссылка {other:?} — не пул менеджера, шаг {} не строится",
                f.id,
                k.name()
            )),
        };
    }
    match pool {
        PoolRef::Contract(a) => Ok((k, *a)),
        // Пара общего контракта (WooPPV2, решение 122 F, 3): исполнитель вызывает сам общий контракт (шаг WOO).
        PoolRef::Member { contract, .. } => Ok((k, *contract)),
        // Решение 295: пара многомонетного пула — шаг по контракту пула (индексы/токены — из записи пары).
        PoolRef::Multi { contract, .. } => Ok((k, *contract)),
        // Решение 447: пул Ekubo — в ядре; шаг EKUBO открывает само ядро (lock), адрес шага — ядро.
        PoolRef::Singleton { manager, .. } if k == ExecKind::Ekubo || f.variant.native_market_module() && k == ExecKind::Balancer => Ok((k, *manager)),
        PoolRef::Singleton { .. } => Err(format!(
            "{}: пул внутри менеджера исполнитель не вызывает",
            f.id
        )),
    }
}

/// Ревью t181 (Н1): supported + у шага пула менеджера (UNI_V4 / INF_*) — ключ в записи реестра есть, обе валюты ключа —
/// ERC-20 (нативная валюта address 0 — не шаг исполнителя, контракт: «native») и это ровно токены пары записи. Так
/// маршрут через такой пул не признаётся исполнимым исполнителем (а не срывается при сборке шага). Решение 400 / 401:
/// и правило pool_gate.rs (класс принят записью симуляции на живых пулах или пул прошёл пробу через исполнитель кошелька
/// окружения; пул, отвергнутый пробой, — нет); не допущен — причина, маршрут идёт прежним путём (бандл / Universal
/// Router), если он собирается, иначе сигнал. Принимает: окружение (кошелёк), запись реестра. Возвращает: как
/// supported. Ошибок иных нет.
pub fn fits(ctx: &Ctx, e: &Entry) -> Result<(ExecKind, Address), String> {
    let (k, a, hooks) = shape(e)?;
    // Решение 441 (ревью, п. 2): шаг BALANCER_V3 — тоже только по записи о приёмке на живых пулах (pool_gate.rs).
    if k.singleton() || k == ExecKind::BalancerV3 {
        super::pool_gate::admits(ctx, e, k, hooks).map_err(|w| format!("{}: {w}", e.family.id))?;
    }
    // Решение 474: пара через буфер ERC4626 — шаг 27, только у исполнителя с BUFFER_MARK в коде.
    if k == ExecKind::BalancerV3
        && let Some(why) = super::kind_buffer::refusal(ctx, &e.pool.state)
    {
        return Err(format!("{}: {why}", e.family.id));
    }
    Ok((k, a))
}

/// Ревью t186 (Н1): горячий путь котировки (quote/graph_executor.rs all_via, quote/graph_fit.rs S2, mod.rs takes) — то же,
/// что `fits(ctx, e).is_ok()`, без строк и без замков общих структур: форма шага (как supported и shape) и допуск
/// pool_gate::admitted (снимки кошельков и итогов пробы). Равенство с fits — тест tests/bsc_dex_pool_pass_401.rs.
/// Принимает: окружение (кошелёк), запись реестра. Ошибок нет.
pub fn fits_ok(ctx: &Ctx, e: &Entry) -> bool {
    let Some(k) = e.family.executor_kind else {
        return false;
    };
    if !k.singleton() {
        // Решение 441 (ревью, п. 2): BALANCER_V3 — то же правило, что в fits.
        if k == ExecKind::BalancerV3 && !super::pool_gate::admitted(ctx, e, k, Address::ZERO) {
            return false;
        }
        // Решение 474: то же правило шага 27, что в fits.
        if k == ExecKind::BalancerV3 && !super::kind_buffer::ready(ctx, &e.pool.state) {
            return false;
        }
        // Решение 447: у шага EKUBO ссылка — пул в ядре (Singleton), у остальных — не Singleton.
        return matches!(e.pool.reference, PoolRef::Singleton { .. }) == (k == ExecKind::Ekubo || (e.family.variant.native_market_module() && k == ExecKind::Balancer));
    }
    let reference = match &e.pool.reference {
        PoolRef::Singleton { .. } => true,
        PoolRef::Contract(_) => k == ExecKind::InfCl && e.family.proto == Proto::InfinityStable,
        _ => false,
    };
    let Some(key) = e.key.as_ref().filter(|_| reference) else {
        return false;
    };
    let (c0, c1) = match key {
        PoolKey::Uniswap(d) => (d.currency0, d.currency1),
        PoolKey::Infinity(d) => (d.currency0, d.currency1),
    };
    let (t0, t1) = (e.pool.token0, e.pool.token1);
    let pair = (t0 == c0 && t1 == c1) || (t0 == c1 && t1 == c0);
    if c0.is_zero() || c1.is_zero() || !pair {
        return false;
    }
    super::pool_gate::admitted(ctx, e, k, key.hooks())
}

/// Форма шага без правила допуска (решение 401: ею пользуется сама проба пула через исполнитель): supported и у шага
/// пула менеджера — ключ есть, валюты ключа ERC-20 = пара записи. Принимает: запись реестра. Возвращает: вид шага, адрес
/// пула, крючок ключа (у шагов не менеджера — нулевой). Ошибок иных нет.
pub fn shape(e: &Entry) -> Result<(ExecKind, Address, Address), String> {
    let (k, a) = supported(e.family, &e.pool.reference)?;
    if !k.singleton() {
        return Ok((k, a, Address::ZERO));
    }
    let key = e
        .key
        .as_ref()
        .ok_or_else(|| format!("{}: у пула нет ключа PoolKey", e.family.id))?;
    let (c0, c1) = match key {
        PoolKey::Uniswap(d) => (d.currency0, d.currency1),
        PoolKey::Infinity(d) => (d.currency0, d.currency1),
    };
    super::kind_key::check_tokens((c0, c1), e.pool.token0, e.pool.token1)
        .map_err(|w| format!("{}: {w}", e.family.id))?;
    Ok((k, a, key.hooks()))
}

/// Вид шага и адрес пула. Принимает: пул реестра. Ошибки: NoRoute — у семейства нет шага исполнителя, пул
/// менеджера-синглтона, у CURVE нет состояния StableSwap или токен входа не из пары пула; NotConfigured — комиссия V2
/// не задана или не в целых долях 1e-6. `token_in` — токен входа шага (нужен виду CURVE для порядка i → j).
pub fn params(ctx: &Ctx, e: &Entry, token_in: Address) -> GatewayResult<(Params, Address)> {
    if let Some(why) = exec_callback::profile_missing(ctx,e.family) {
        return Err(ctx.err(ErrorCode::NoRoute,why));
    }
    let (k, pool) =
        supported(e.family, &e.pool.reference).map_err(|w| ctx.err(ErrorCode::NoRoute, w))?;
    if let PoolState::Bundles(book)=&e.pool.state {
        if k!=ExecKind::Balancer || pool!=book.market.pool || !book.market.tokens.contains(&token_in){return Err(ctx.err(ErrorCode::NoRoute,"Bundles Swap: форма шага не соответствует рынку"))}
        return Ok((Params::Balancer{pool_id:pool.into_word()},ctx.dex_addr("dex.router",e.family.id)?));
    }
    if let PoolState::Surge(s)=&e.pool.state {
        let module=ctx.dex_addr("dex.router",e.family.id)?;
        if k!=ExecKind::Basin || module!=s.trader || s.token!=pool || (token_in!=s.token && token_in!=s.wrapped){
            return Err(ctx.err(ErrorCode::NoRoute,"Surge: исполнитель, плательщик или валюта изменились"));
        }
        return Ok((Params::Basin,module));
    }
    if let PoolState::Dagrow(s)=&e.pool.state {
        let module=ctx.dex_addr("dex.router",e.family.id)?;
        if k!=ExecKind::Basin || pool!=s.pool || module!=s.payer || (token_in!=s.stable && token_in!=s.dagr){
            return Err(ctx.err(ErrorCode::NoRoute,"DAGrow: плательщик или пара изменились"));
        }
        return Ok((Params::Basin,module));
    }
    if e.family.variant == crate::venues::bsc_dex::pools::family_variant::Variant::Temple {
        let PoolState::Temple(m)=&e.pool.state else{return Err(ctx.err(ErrorCode::NoRoute,"TempleDAO: состояние пары отсутствует"));};
        if k!=ExecKind::Basin || m.pair!=pool || (token_in!=e.pool.token0 && token_in!=e.pool.token1) ||
           (token_in!=m.temple && token_in!=m.stable) {
            return Err(ctx.err(ErrorCode::NoRoute,"TempleDAO: для входа отсутствует зарегистрированный обмен"));
        }
        return Ok((Params::Basin,ctx.dex_addr("dex.router",e.family.id)?));
    }
    if e.family.variant == crate::venues::bsc_dex::pools::family_variant::Variant::CandySwap {
        let PoolState::CandySwap(s)=&e.pool.state else {return Err(ctx.err(ErrorCode::NoRoute,"CandySwap: состояние пары не установлено"));};
        if token_in!=e.pool.token0 && token_in!=e.pool.token1 {return Err(ctx.err(ErrorCode::NoRoute,"CandySwap: неизвестный вход"));}
        let i=usize::from(token_in!=e.pool.token0);
        if k==ExecKind::Basin && e.family.tradable {
            return Ok((Params::Basin,ctx.dex_addr("dex.router",e.family.id)?));
        }
        if k!=ExecKind::V2 || s.balances[i]>s.reserves[i] {
            return Err(ctx.err(ErrorCode::NoRoute,"CandySwap: для избыточного входного остатка требуется модуль с учётом средств плательщика"));
        }
    }
    if e.family.variant.protected_router_wallet() {
        if k!=ExecKind::Basin || !matches!(&e.pool.state,PoolState::OraclePmm(_)) || (token_in!=e.pool.token0 && token_in!=e.pool.token1){
            return Err(ctx.err(ErrorCode::NoRoute,"SafeSwap Trade: требуется подтверждённый рынок и его входной токен"));
        }
        return Ok((Params::Basin,ctx.dex_addr("dex.router",e.family.id)?));
    }
    if matches!(e.family.variant,super::super::super::pools::family_variant::Variant::MaverickOne | super::super::super::pools::family_variant::Variant::Pendle) {
        if k!=ExecKind::Balancer || !matches!(e.pool.state,PoolState::OraclePmm(_)) || (token_in!=e.pool.token0 && token_in!=e.pool.token1){return Err(ctx.err(ErrorCode::NoRoute,"Для модуля требуется подтверждённая сторона рынка"));}
        return Ok((Params::Balancer{pool_id:pool.into_word()},ctx.dex_addr("dex.router",e.family.id)?));
    }
    if e.family.variant.umbrae() {
        if k!=ExecKind::Balancer || !matches!(&e.pool.state,PoolState::UmbraeDamm(_) | PoolState::OraclePmm(_)) || (token_in!=e.pool.token0 && token_in!=e.pool.token1) {return Err(ctx.err(ErrorCode::NoRoute,"Umbrae: неподтверждённый рынок или вход"));}
        return Ok((Params::Balancer{pool_id:pool.into_word()},ctx.dex_addr("dex.router",e.family.id)?));
    }
    if e.family.variant.registered_module() {
        // Wire 17 takes a bytes32 poolId. Its destination is the factory-bound module and
        // its right-aligned id is the exact V3 pool. Balancer pool state is decoded below.
        if k!=ExecKind::Balancer || !matches!(&e.pool.state,PoolState::V3{..})
            || (token_in!=e.pool.token0 && token_in!=e.pool.token1) {
            return Err(ctx.err(ErrorCode::NoRoute,"Прямой модуль V3: не подтверждены рынок и вход"));
        }
        return Ok((Params::Balancer{pool_id:pool.into_word()},ctx.dex_addr("dex.router",e.family.id)?));
    }
    if e.family.variant.native_market_module() {
        let PoolRef::Singleton { id, .. } = e.pool.reference else {
            return Err(ctx.err(ErrorCode::NoRoute, "o1: требуется физический PoolId"));
        };
        let supported = match e.family.variant {
            crate::venues::bsc_dex::pools::family_variant::Variant::FluxNativeCl => matches!(e.pool.state, PoolState::Cl { .. }),
            crate::venues::bsc_dex::pools::family_variant::Variant::FluxNativeBin => matches!(e.pool.state, PoolState::Bin { .. }),
            _ => matches!(e.pool.state, PoolState::OraclePmm(_)),
        };
        if k != ExecKind::Balancer || !supported
            || e.key.as_ref().is_none_or(|key| key.id() != id)
            || (token_in != e.pool.token0 && token_in != e.pool.token1) {
            return Err(ctx.err(ErrorCode::NoRoute, "o1: не подтверждены ключ и направление WETH"));
        }
        return Ok((Params::Balancer { pool_id: id }, ctx.dex_addr("dex.router", e.family.id)?));
    }
    if super::super::super::quote::vista_pricing::is_vista(e) {
        if k != ExecKind::Basin || !matches!(&e.pool.state,PoolState::EtherVista(_))
            || (token_in!=e.pool.token0 && token_in!=e.pool.token1) {
            return Err(ctx.err(ErrorCode::NoRoute,"EtherVista: неверная пара или версия шага V5"));
        }
        return Ok((Params::Basin,ctx.dex_addr("dex.router",e.family.id)?));
    }
    if super::super::super::quote::oasis_pricing::is_oasis(e) {
        if k != ExecKind::Basin
            || !matches!(e.pool.state, PoolState::Oasis { .. })
            || (token_in != e.pool.token0 && token_in != e.pool.token1)
        {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "OasisSwap: не подтверждены форма и адреса шага V5",
            ));
        }
        return Ok((Params::Basin, ctx.dex_addr("dex.router", e.family.id)?));
    }
    if e.family.variant == crate::venues::bsc_dex::pools::family_variant::Variant::BunnySwap {
        if k!=ExecKind::Basin || !matches!(e.pool.state,PoolState::V2{..}) || (token_in!=e.pool.token0 && token_in!=e.pool.token1) {
            return Err(ctx.err(ErrorCode::NoRoute,"RabbitRouter: форма шага или актив не подтверждены"));
        }
        return Ok((Params::Basin,ctx.dex_addr("dex.router",e.family.id)?));
    }
    if e.family.variant == crate::venues::bsc_dex::pools::family_variant::Variant::Memebox {
        if k != ExecKind::Basin || !matches!(e.pool.state,PoolState::Memebox(_)) {
            return Err(ctx.err(ErrorCode::NoRoute,"Memebox: отсутствует подтверждённый шаг модуля"));
        }
        return Ok((Params::Basin,ctx.dex_addr("dex.router",e.family.id)?));
    }
    if e.family.variant == crate::venues::bsc_dex::pools::family_variant::Variant::FraxTwamm {
        if k != ExecKind::Basin
            || !matches!(e.pool.state, PoolState::V2 { .. })
            || (token_in != e.pool.token0 && token_in != e.pool.token1)
        {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Fraxswap: неподтверждённая форма шага V5",
            ));
        }
        return Ok((Params::Basin, ctx.dex_addr("dex.router", e.family.id)?));
    }
    let p = match k {
        ExecKind::V2 => Params::V2 {
            fee_pips: fee_pips(ctx, e)?,
        },
        ExecKind::V3 => Params::V3,
        ExecKind::Algebra => Params::Algebra,
        ExecKind::Elastic => Params::Elastic,
        // Решение 273: фабрика пары на паузе — обмен откатился бы, шаг не строится.
        ExecKind::Solidly => match &e.pool.state {
            PoolState::Solidly { gate, .. } if gate.paused => {
                let why = format!(
                    "{}: фабрика пары на паузе (isPaused) — шаг не строится",
                    e.family.id
                );
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
            // Решение 470 (T470-06): пул Aero Basic у исполнителя v5 — шаг AERO_BASIC (выход с MEV-налогом этой
            // транзакции; pools/exec_kind.rs for_wallet); у v1–v4 — SOLIDLY (налог > 0 — отказ, pools/aero_mev.rs fresh).
            _ if exec_kind::for_wallet(ctx, e.family) == Some(ExecKind::AeroBasic) => {
                Params::AeroBasic
            }
            _ => Params::Solidly,
        },
        ExecKind::Dodo => Params::Dodo,
        ExecKind::Woo => Params::Woo,
        ExecKind::Maverick => Params::Maverick,
        // Решение 284: пара Nomiswap Stable — шаг без данных; остановленная пара (состояние не прочитано) не исполняется.
        ExecKind::NomiStable => match &e.pool.state {
            PoolState::StableSwap(d) if !d.halted => Params::NomiStable,
            _ => {
                let why = format!("{}: у пары нет состояния Nomiswap Stable", e.family.id);
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 285: пул Metric — направление в порядке токенов самого пула (OraclePmmData.pool_token0, load_metric.rs).
        ExecKind::Metric => match &e.pool.state {
            PoolState::OraclePmm(d) if d.pool_token0.is_some() => Params::Metric {
                zero_for_one: d.pool_token0 == Some(token_in),
            },
            _ => {
                let why = format!("{}: у пула Metric не прочитан порядок токенов", e.family.id);
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 286: iZiSwap / Biswap V3 — направление контракт берёт сам (tokenIn < tokenOut → swapX2Y); данных нет.
        // Ревью t118: шаг строится только у прочитанного пула, у которого tokenX = token0 пары записи (load_izi.rs) —
        // на этом порядке стоит выбор swapX2Y / swapY2X в контракте.
        ExecKind::Izi => match &e.pool.state {
            PoolState::OraclePmm(d) if d.pool_token0 == Some(e.pool.token0) => Params::Izi,
            _ => {
                let why = format!(
                    "{}: у пула iZiSwap не прочитано состояние (tokenX)",
                    e.family.id
                );
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 287: пул KyberSwap DMM — шаг без данных; без прочитанного состояния AmpV2 не строится.
        ExecKind::KyberDmm => match &e.pool.state {
            PoolState::AmpV2(_) => Params::KyberDmm,
            _ => {
                let why = format!("{}: у пула нет состояния KyberSwap DMM", e.family.id);
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 288: пара SmarDex — направление контракт берёт по порядку адресов (пара сортирует токены); без
        // прочитанного состояния Fictive шаг не строится.
        ExecKind::Smardex => match &e.pool.state {
            PoolState::Fictive(_) => Params::Smardex,
            _ => {
                let why = format!("{}: у пары нет состояния SmarDex", e.family.id);
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 289: пул Balancer V2 — шаг вызывает хранилище (dex.vault), data — id пула из прочитанного состояния;
        // пул на паузе не исполняется.
        ExecKind::Balancer => match &e.pool.state {
            PoolState::VaultPool(d) if !d.paused => {
                let vault = ctx.dex_addr("dex.vault", e.family.id)?;
                return Ok((Params::Balancer { pool_id: d.pool_id }, vault));
            }
            _ => {
                let why = format!(
                    "{}: у пула Balancer нет состояния или он на паузе",
                    e.family.id
                );
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 441: пул Balancer V3 — шаг вызывает хранилище V3 (dex.vault), data — адрес пула; пул на паузе (или
        // хранилище на паузе, пул не инициализирован — paused при чтении) не исполняется.
        ExecKind::BalancerV3 => match &e.pool.state {
            PoolState::VaultPool(d) if !d.paused && d.v3.is_some() => {
                let vault = ctx.dex_addr("dex.vault", e.family.id)?;
                // Решение 474: сторона пары через буфер ERC4626 — шаг BALANCER_V3_BUF (kind_buffer.rs).
                let zero_in = token_in == e.pool.token0;
                let p = super::kind_buffer::params(d, pool, zero_in)
                    .unwrap_or(Params::BalancerV3 { pool });
                return Ok((p, vault));
            }
            _ => {
                let why = format!("{}: пул Balancer V3 не прочитан или на паузе", e.family.id);
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 290: пул Liquidity Book V2.1 — шаг без данных (направление контракт берёт по getTokenX пула); без
        // прочитанного состояния LbBin не строится.
        ExecKind::Lb => match &e.pool.state {
            PoolState::LbBin(_) => Params::Lb,
            _ => {
                let why = format!("{}: у пула нет состояния Liquidity Book", e.family.id);
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 440: пул Fluid DEX T1 — направление в порядке токенов самого пула (SmartColDebtData.token0), нативная
        // монета пула на стороне входа / выхода и слой ликвидности — из прочитанного состояния; пул на паузе или токен
        // входа не из пула (ревью 440) — шаг не строится.
        ExecKind::Fluid => match &e.pool.state {
            PoolState::SmartColDebt(d) if !d.paused && d.has(token_in) => {
                Params::Fluid(fluid_step(d, token_in))
            }
            _ => {
                let why = format!(
                    "{}: Fluid — нет состояния, пауза или чужой вход",
                    e.family.id
                );
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 486: пул Shell Protocol Proteus — шаг вызывает учёт Ocean (ocean() пула, проверен = dex.vault), data —
        // пул, нативная сторона учёта на входе / выходе и знаки токена входа (shell_step); заморожен или вход не из пула —
        // шаг не строится.
        ExecKind::Shell => match &e.pool.state {
            PoolState::Proteus(d) if !d.frozen && d.is_x(token_in).is_some() => {
                return shell_step(d, pool, token_in).map_err(|w| ctx.err(ErrorCode::NoRoute, w));
            }
            _ => {
                let why = format!(
                    "{}: Shell — нет состояния, заморожен, чужой вход",
                    e.family.id
                );
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 445: пул Liquidity Book v2.0 — шаг без данных (направление контракт берёт по tokenX() пары).
        ExecKind::Lb20 => match &e.pool.state {
            PoolState::LbBin(d) if d.v20 => Params::Lb20,
            _ => {
                let why = format!("{}: у пула нет состояния Liquidity Book v2.0", e.family.id);
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 447: пул Ekubo — config ключа из прочитанного состояния (токены ключа — токены шага по порядку адресов).
        // Решение 454: нативный ETH ключа и forward — только у MEVCapture (pools/ekubo_ext.rs).
        ExecKind::Ekubo => match &e.pool.state {
            PoolState::Ekubo(d) => Params::Ekubo {
                config: d.config,
                native: d.native.is_some(),
                forward: d.mev.is_some(),
            },
            _ => {
                let why = format!("{}: у пула нет состояния Ekubo", e.family.id);
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        ExecKind::UniV4 | ExecKind::InfCl | ExecKind::InfBin => {
            return singleton(ctx, e, k);
        }
        // Решение 442: crypto-пул Curve (Twocrypto-NG / Tricrypto-NG) — шаг STABLE_U256, exchange(uint256 i, uint256 j,
        // dx, 0); индексы — из прочитанного состояния Crypto (crypto_step).
        ExecKind::StableU256 if matches!(e.pool.state, PoolState::Crypto(_)) => {
            crypto_step(e, token_in).map_err(|w| ctx.err(ErrorCode::NoRoute, w))?
        }
        // Решение 472: CURVE_ETH как вид семейства не встречается (вид — CURVE, шаг с ETH выбирает stable) — сюда тоже.
        ExecKind::Curve | ExecKind::CurveEth | ExecKind::StableU256 | ExecKind::Saddle => {
            let ver = ctx.exec_info().and_then(|(_, v)| v);
            stable(e, k, token_in, stable_native::wrapped(ctx), ver)
                .map_err(|w| ctx.err(ErrorCode::NoRoute, w))?
        }
        // Решение 484: пул Wombat — шаг без данных (approve + swap пула); пул на паузе или актив монеты входа на паузе
        // (StableData.wombat.asset_paused) — обмен откатился бы, шаг не строится.
        ExecKind::Wombat => match &e.pool.state {
            PoolState::StableSwap(d) if !d.halted && wombat_open(d, e, token_in) => Params::Wombat,
            _ => {
                let why = format!("{}: Wombat не прочитан или на паузе", e.family.id);
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 485: скважина Basin — шаг без данных (swapFrom по токенам шага); без прочитанного состояния BasinWell
        // или с токеном входа не из скважины шаг не строится.
        ExecKind::Basin => match &e.pool.state {
            PoolState::BasinWell(d) if d.has(token_in) => Params::Basin,
            _ => {
                let why = format!(
                    "{}: скважина Basin — нет состояния или чужой вход",
                    e.family.id
                );
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
        // Решение 470 (T470-06): вид семейства aero_basic — SOLIDLY (ветка выше выбирает AERO_BASIC у кошелька v5);
        // как вид семейства AERO_BASIC не встречается — сюда тоже на случай такой пометки (пауза — та же проверка).
        ExecKind::AeroBasic => match &e.pool.state {
            PoolState::Solidly { gate, .. } if !gate.paused => Params::AeroBasic,
            _ => {
                let why = format!("{}: пул Aero Basic без состояния или на паузе", e.family.id);
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
        },
    };
    Ok((p, pool))
}

/// Решение 484: монета входа — из пары пула, и её актив не на паузе (PoolV2.swap requireAssetNotPaused(fromToken)).
/// Принимает: состояние StableSwap пула Wombat, запись, токен входа. Ошибок нет (нет части Wombat — false).
fn wombat_open(d: &StableData, e: &Entry, token_in: Address) -> bool {
    let i = if token_in == e.pool.token0 {
        d.i0
    } else if token_in == e.pool.token1 {
        d.i1
    } else {
        return false;
    };
    d.wombat
        .as_ref()
        .and_then(|w| w.asset_paused.get(usize::from(i)))
        .is_some_and(|p| !*p)
}

/// Решение 440: параметры шага FLUID. Принимает: состояние пула, токен входа шага (в паре — обёртка нативной монеты).
/// Возвращает: направление по token0 пула и где нативная монета (1 — вход, 2 — выход, 0 — нет). Ошибок нет.
fn fluid_step(d: &SmartColDebtData, token_in: Address) -> FluidStep {
    let zero_for_one = token_in == d.token0;
    let (native_in, native_out) = if zero_for_one {
        (d.native0, d.native1)
    } else {
        (d.native1, d.native0)
    };
    let native = if native_in {
        1
    } else if native_out {
        2
    } else {
        0
    };
    FluidStep {
        zero_for_one,
        native,
        liquidity: d.liquidity,
    }
}

/// Решение 395: шаг пула внутри менеджера. Ключ — из реестра (Entry.key, его же берёт Universal Router — exec/swap_v4.rs),
/// сверка с ссылкой: Singleton — PoolId ключа = id и менеджер ключа (у Infinity) = менеджер ссылки; Contract (Infinity
/// Stable) — крючок ключа = адрес пула. Пул шага: UNI_V4 — PoolManager (адрес ссылки), INF_* — Vault (dex.vault DEX).
/// Принимает: окружение, пул, вид. Возвращает: параметры и адрес пула шага. Ошибки: NoRoute — нет ключа, ключ не того
/// вида или не сходится со ссылкой; NotConfigured — dex.vault не задана.
fn singleton(ctx: &Ctx, e: &Entry, k: ExecKind) -> GatewayResult<(Params, Address)> {
    let no = |w: String| ctx.err(ErrorCode::NoRoute, format!("{}: {w}", e.family.id));
    let key = e
        .key
        .as_ref()
        .ok_or_else(|| no("у пула нет ключа PoolKey".into()))?;
    // Решения 400 / 401: то же правило, что fits (котировка и сделка — одно правило, pool_gate.rs).
    super::pool_gate::admits(ctx, e, k, key.hooks()).map_err(no)?;
    singleton_step(ctx, e, k)
}

/// Шаг пула менеджера без правила допуска (решение 401: проба пула через исполнитель — check/pass_probe.rs; сделка и
/// котировка идут через singleton). Принимает: окружение, пул, вид. Возвращает: параметры и адрес пула шага. Ошибки: как
/// у singleton, кроме допуска.
pub fn singleton_step(ctx: &Ctx, e: &Entry, k: ExecKind) -> GatewayResult<(Params, Address)> {
    let no = |w: String| ctx.err(ErrorCode::NoRoute, format!("{}: {w}", e.family.id));
    let key = e
        .key
        .as_ref()
        .ok_or_else(|| no("у пула нет ключа PoolKey".into()))?;
    match &e.pool.reference {
        PoolRef::Singleton { manager, id } => {
            if key.id() != *id {
                return Err(no(format!("PoolId ключа {} ≠ id пула {id}", key.id())));
            }
            if let PoolKey::Infinity(d) = key
                && d.manager != *manager
            {
                return Err(no(format!(
                    "менеджер ключа {:#x} ≠ менеджер пула {manager:#x}",
                    d.manager
                )));
            }
        }
        // Ревью t181 (Н2): Contract — только Infinity Stable (шаг INF_CL): крючок ключа = адрес пула, менеджер ключа =
        // dex.pool_manager этой DEX (CLPoolManager); иначе шаг не строится.
        PoolRef::Contract(hook)
            if k == ExecKind::InfCl && e.family.proto == Proto::InfinityStable =>
        {
            let PoolKey::Infinity(d) = key else {
                return Err(no("ключ пула Infinity Stable не того вида".into()));
            };
            if d.hooks != *hook {
                return Err(no(format!(
                    "крючок ключа {:#x} ≠ адрес пула {hook:#x}",
                    d.hooks
                )));
            }
            let pm = ctx.dex_addr("dex.pool_manager", e.family.id)?;
            if d.manager != pm {
                return Err(no(format!(
                    "менеджер ключа {:#x} ≠ dex.pool_manager {pm:#x}",
                    d.manager
                )));
            }
        }
        other => return Err(no(format!("ключ пула не сходится со ссылкой {other:?}"))),
    }
    match (k, key) {
        (ExecKind::UniV4, PoolKey::Uniswap(d)) => Ok((
            Params::UniV4 { key: V4Key::of(d) },
            e.pool.reference.address(),
        )),
        (ExecKind::InfCl | ExecKind::InfBin, PoolKey::Infinity(d)) => {
            let vault = ctx.dex_addr("dex.vault", e.family.id)?;
            let key = InfKey::of(d);
            let p = if k == ExecKind::InfCl {
                Params::InfCl { key }
            } else {
                Params::InfBin { key }
            };
            Ok((p, vault))
        }
        _ => Err(no(format!("ключ пула не того вида для шага {}", k.name()))),
    }
}

/// Решение 442: индексы шага STABLE_U256 crypto-пула Curve — i монета входа, j выхода. Принимает: пул реестра, токен
/// входа. Возвращает: параметры шага или причину (состояние не Crypto; обмены пула остановлены; токен входа не из пары).
fn crypto_step(e: &Entry, token_in: Address) -> Result<Params, String> {
    let PoolState::Crypto(d) = &e.pool.state else {
        return Err(format!(
            "{}: у пула нет состояния crypto-пула Curve",
            e.family.id
        ));
    };
    if d.halted {
        return Err(format!(
            "{}: обмены пула остановлены — шаг не строится",
            e.family.id
        ));
    }
    if token_in == e.pool.token0 {
        Ok(Params::StableU256 { i: d.i0, j: d.i1 })
    } else if token_in == e.pool.token1 {
        Ok(Params::StableU256 { i: d.i1, j: d.i0 })
    } else {
        Err(format!(
            "{}: токен входа {token_in} не из пары пула",
            e.family.id
        ))
    }
}

/// Индексы шага StableSwap (CURVE, STABLE_U256, SADDLE): i — монета входа, j — выхода. Принимает: пул реестра, вид,
/// токен входа. Возвращает: параметры шага или причину (состояние не StableSwap; обмены пула остановлены — пауза /
/// kill / сбой проверки цены Lista; в паре — сама метка 0xEeee…; нативная монета пары у вида без шага с BNB; токен входа
/// не token0/token1 пула). Решение 472: `ver` — сверенная версия исполнителя кошелька (None — не сверена): шаг
/// CURVE_ETH — только у v5.
fn stable(
    e: &Entry,
    k: ExecKind,
    token_in: Address,
    wrapped: Option<Address>,
    ver: Option<Version>,
) -> Result<Params, String> {
    let PoolState::StableSwap(d) = &e.pool.state else {
        return Err(format!("{}: у пула нет состояния StableSwap", e.family.id));
    };
    if d.halted {
        return Err(format!("{}: обмены пула остановлены", e.family.id));
    }
    // Метка BNB_ADDRESS (0xEeee…) в паре быть не должна: загрузка ставит вместо неё обёртку (pools/stable_native.rs);
    // токеном шага она быть не может — отказ до отправки.
    if [e.pool.token0, e.pool.token1].contains(&STABLE_NATIVE) {
        return Err(format!(
            "{}: в паре метка нативной монеты 0xEeee… (не ERC-20) — шаг не строится",
            e.family.id
        ));
    }
    let (i, j) = if token_in == e.pool.token0 {
        (d.i0, d.i1)
    } else if token_in == e.pool.token1 {
        (d.i1, d.i0)
    } else {
        return Err(format!(
            "{}: токен входа {token_in} не из пары пула",
            e.family.id
        ));
    };
    // Нативная монета пула — монета этой пары (вход или выход шага): шаг с BNB (только PancakeSwap Stable / Lista).
    if let Some(n) = d.native.filter(|n| *n == i || *n == j) {
        // Ревью 7b4ea25 (🟡1): сторона BNB шага — текущая обёртка dex.native_wrapped. Настройку могли сменить на лету, а
        // пара пула сопоставлена при загрузке: иначе контракт вызвал бы withdraw/deposit не у той обёртки (откат).
        let bnb_side = if n == i {
            token_in
        } else if token_in == e.pool.token0 {
            e.pool.token1
        } else {
            e.pool.token0
        };
        if wrapped != Some(bnb_side) {
            return Err(format!(
                "{}: сторона BNB пары {bnb_side} не совпадает с dex.native_wrapped ({wrapped:?}) — пул перечитается",
                e.family.id
            ));
        }
        return match k {
            ExecKind::StableU256 => Ok(Params::StableU256Bnb {
                i,
                j,
                bnb_in: n == i,
            }),
            // Решение 472 (T472-08): пул Curve с нативным ETH — шаг CURVE_ETH, только исполнитель v5.
            ExecKind::Curve | ExecKind::CurveEth => curve_eth(e, i, j, n == i, ver),
            _ => Err(format!(
                "{}: монета пары — нативный BNB пула, у вида {} шага с BNB нет",
                e.family.id,
                k.name()
            )),
        };
    }
    Ok(match k {
        ExecKind::StableU256 => Params::StableU256 { i, j },
        ExecKind::Saddle => Params::Saddle { i, j },
        _ => Params::Curve {
            i: i128::from(i),
            j: i128::from(j),
        },
    })
}

/// Решение 472 (T472-08): шаг CURVE_ETH (пул Curve с нативным ETH). Принимает: пул, индексы i → j, вход ли нативная
/// монета, сверенную версию исполнителя кошелька. Возвращает: параметры шага или причину (версия не сверена или ниже
/// v5 — шага CURVE_ETH у исполнителя нет).
fn curve_eth(
    e: &Entry,
    i: u8,
    j: u8,
    eth_in: bool,
    ver: Option<Version>,
) -> Result<Params, String> {
    let k = ExecKind::CurveEth;
    let Some(v) = ver else {
        return Err(format!(
            "{}: монета пары — нативный ETH пула Curve; версия исполнителя кошелька не сверена — шаг {} есть только с \
             исполнителя {}",
            e.family.id,
            k.name(),
            exec_callback::since(k).label()
        ));
    };
    match exec_callback::step_absent(k, v) {
        None => Ok(Params::CurveEth {
            i: i128::from(i),
            j: i128::from(j),
            eth_in,
        }),
        Some(why) => Err(format!("{}: нативный ETH пула Curve: {why}", e.family.id)),
    }
}

/// Комиссия пары V2 в долях 1e-6: своя у пары (Pool.fee_pips) или dex.v2_fee_pct DEX × 10000. Ошибки: NotConfigured —
/// не задана или не целое число долей 1e-6 (обрезать комиссию нельзя: пара откатит обмен по проверке K).
fn fee_pips(ctx: &Ctx, e: &Entry) -> GatewayResult<u32> {
    let pct = pool_quote::v2_fee_pct(ctx, e)?;
    let pips = pct * Decimal::from(10_000u32);
    if pips.fract() != Decimal::ZERO || pips < Decimal::ZERO {
        return Err(ctx.not_set(
            "dex.v2_fee_pct",
            format!("{}: комиссия {pct}% не в целых долях 1e-6", e.family.id),
        ));
    }
    pips.to_u32().ok_or_else(|| {
        ctx.not_set(
            "dex.v2_fee_pct",
            format!("{}: комиссия {pct}% вне диапазона", e.family.id),
        )
    })
}

/// Решение 486: шаг SHELL. Принимает: состояние пула Proteus, адрес пула (прокси — primitive учёта), токен входа.
/// Возвращает: (параметры, учёт Ocean — контракт шага) или причину (токен не из пула, знаков входа больше 18 — учёт
/// округлил бы вход с пылью).
fn shell_step(
    d: &ProteusData,
    pool: Address,
    token_in: Address,
) -> Result<(Params, Address), String> {
    let in_x = d.is_x(token_in).ok_or("Shell: токен входа не из пула")?;
    let (native_in, native_out, dec_in) = if in_x {
        (d.native_x, d.native_y, d.dec_x)
    } else {
        (d.native_y, d.native_x, d.dec_y)
    };
    if dec_in > 18 {
        return Err(format!(
            "Shell: у токена входа {dec_in} знаков (> 18) — вход в учёт не точен"
        ));
    }
    let native = if native_in {
        1
    } else if native_out {
        2
    } else {
        0
    };
    Ok((
        Params::Shell(ShellStep {
            primitive: pool,
            native,
            dec_in,
        }),
        d.ocean,
    ))
}
