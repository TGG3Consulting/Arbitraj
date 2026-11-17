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
//! стейблсвапах)»): вид STABLE_U256_BNB (bnb_in — по тому, вход это или выход); у CURVE / SADDLE — отказ.
//! Решение 273: пул Solidly с фабрикой на паузе — шаг не строится (NoRoute с причиной).
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

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::ctx::Ctx;
use super::super::super::pools::exec_kind::ExecKind;
use super::super::super::pools::families::{Family, Proto};
use super::super::super::pools::key::PoolKey;
use super::super::super::pools::registry::Entry;
use super::super::super::pools::stable_native;
use super::super::super::quote::pool_quote;
use super::kind::{InfKey, Params, STABLE_NATIVE, V4Key};

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
    if k.singleton() {
        super::pool_gate::admits(ctx, e, k, hooks).map_err(|w| format!("{}: {w}", e.family.id))?;
    }
    Ok((k, a))
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
    let (k, pool) =
        supported(e.family, &e.pool.reference).map_err(|w| ctx.err(ErrorCode::NoRoute, w))?;
    let p = match k {
        ExecKind::V2 => Params::V2 {
            fee_pips: fee_pips(ctx, e)?,
        },
        ExecKind::V3 => Params::V3,
        ExecKind::Algebra => Params::Algebra,
        // Решение 273: фабрика пары на паузе — обмен откатился бы, шаг не строится.
        ExecKind::Solidly => match &e.pool.state {
            PoolState::Solidly { gate, .. } if gate.paused => {
                let why = format!(
                    "{}: фабрика пары на паузе (isPaused) — шаг не строится",
                    e.family.id
                );
                return Err(ctx.err(ErrorCode::NoRoute, why));
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
                let why = format!("{}: у пула iZiSwap не прочитано состояние (tokenX)", e.family.id);
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
                let why = format!("{}: у пула Balancer нет состояния или он на паузе", e.family.id);
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
        ExecKind::UniV4 | ExecKind::InfCl | ExecKind::InfBin => {
            return singleton(ctx, e, k);
        }
        ExecKind::Curve | ExecKind::StableU256 | ExecKind::Saddle => {
            stable(e, k, token_in, stable_native::wrapped(ctx))
                .map_err(|w| ctx.err(ErrorCode::NoRoute, w))?
        }
    };
    Ok((p, pool))
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

/// Индексы шага StableSwap (CURVE, STABLE_U256, SADDLE): i — монета входа, j — выхода. Принимает: пул реестра, вид,
/// токен входа. Возвращает: параметры шага или причину (состояние не StableSwap; обмены пула остановлены — пауза /
/// kill / сбой проверки цены Lista; в паре — сама метка 0xEeee…; нативная монета пары у вида без шага с BNB; токен входа
/// не token0/token1 пула).
fn stable(
    e: &Entry,
    k: ExecKind,
    token_in: Address,
    wrapped: Option<Address>,
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
