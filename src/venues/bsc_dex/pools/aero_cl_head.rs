//! Файл: venues/bsc_dex/pools/aero_cl_head.rs
//! Что делает: решение 470 (T470-01, T470-07) — комиссия пулов Aero Slipstream V3 на блоке ПАЧКОЙ (без MEV-налога):
//! наибольшая из «зафиксированной» (fee() пула на блоке — blockFee блока или первая комиссия на его время) и «первой
//! комиссии СЛЕДУЮЩЕГО блока» по порту крючка (aero_first_fee.rs) для времени блока + δ, δ = 0…dex.cl_hook_ahead_s.
//! Два прохода (node/call.rs try_chunks частями по dex.load_batch): (1) параметры крючка secondsAgo,
//! defaultScalingFactor, defaultFeeCap; по пулу — fee(), slot0(), dynamicFeeConfig(пул), tickSpacingToFee(шаг) фабрики;
//! (2) по пулу — observations((index + 1) % cardinality), observations(0), observe([secondsAgo − δ, 0]) на каждое δ.
//! Время блока — load_smardex.rs block_secs (одно на блок, память).
//! Кто вызывает: pools/aero_cl.rs (полное чтение и перед отправкой), pools/per_block_aero.rs (каждым блоком).
//! От чего зависит: ctx.rs, node/call.rs, pools/{aero_first_fee, load_batch, load_smardex}.rs.
//! Настройки: dex.cl_swap_hook, dex.factory, dex.cl_hook_ahead_s [DEX]; dex.load_batch [площадка DEX].
//!
//! Подписи — Dromos-Labs/metadex-slipstream-public cf1ae46f: ICLPoolState.sol L21-L31 (slot0), L157-L160 (observations),
//! ICLPoolDerivedState (observe), CLPool.sol L190-L194 (fee), ICLFactory.sol L194 (tickSpacingToFee);
//! metadex-public 79c8d5b6 IDynamicSwapFeeHook.sol L211, L215, L219, L232-L235.

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::{I24, I56};
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call::{self, Piece};
use super::aero_first_fee::{self as ff, Config, Defaults, Head, Obs};
use super::families::Family;
use super::{load_batch, load_smardex};

alloy::sol! {
    interface IAeroHookFee {
        function secondsAgo() external view returns (uint32);
        function defaultScalingFactor() external view returns (uint256);
        function defaultFeeCap() external view returns (uint256);
        function dynamicFeeConfig(address pool)
            external
            view
            returns (uint24, uint24, uint64, bool, uint24);
    }

    interface IAeroClOracle {
        function fee() external view returns (uint24);
        function slot0() external view returns (uint160, int24, uint16, uint16, uint16, bool);
        function observations(uint256 index) external view returns (uint32, int56, uint160, bool);
        function observe(uint32[] secondsAgos) external view returns (int56[], uint160[]);
    }

    interface IAeroClTickFee {
        function tickSpacingToFee(int24 tickSpacing) external view returns (uint24);
    }
}

/// Настройка: на сколько секунд вперёд от времени блока считать первую комиссию следующего блока.
pub const AHEAD: &str = "dex.cl_hook_ahead_s";

/// Число вызовов первого прохода на пул.
const FIRST: usize = 4;

/// Ответ вызова пачки по ABI функции C. Ошибки (текст): сбой узла, откат, не по ABI.
fn dec<C: SolCall>(p: Option<&Piece>) -> Result<C::Return, String> {
    let b = match p {
        Some(Ok(Ok(b))) => b,
        Some(Ok(Err(why))) => return Err(format!("{}: откат {why}", C::SIGNATURE)),
        Some(Err(why)) => return Err(format!("{}: узел {why}", C::SIGNATURE)),
        None => return Err(format!("{}: нет ответа", C::SIGNATURE)),
    };
    C::abi_decode_returns(b).map_err(|e| format!("{}: не по ABI {e}", C::SIGNATURE))
}

/// Вызов для пачки. Ошибок нет.
fn ask<C: SolCall>(to: Address, c: &C) -> (Address, Bytes) {
    (to, Bytes::from(c.abi_encode()))
}

/// Комиссии пулов на блоке (без MEV-налога), пипсы 1e6. Принимает: окружение, вид (id DEX), пулы (адрес, tickSpacing),
/// номер блока. Возвращает: по пулу комиссию или причину (пул не прочитан). Ошибки: не заданы dex.cl_swap_hook,
/// dex.factory, dex.cl_hook_ahead_s, dex.load_batch; время блока не прочитано; параметры крючка не прочитаны.
pub async fn read(
    ctx: &Ctx,
    f: &Family,
    pools: &[(Address, i32)],
    block: u64,
) -> GatewayResult<Vec<Result<u32, String>>> {
    let hook = ctx.dex_addr("dex.cl_swap_hook", f.id)?;
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let ahead = u32::try_from(ctx.dex_int(AHEAD, f.id)?).unwrap_or(u32::MAX);
    let size = load_batch::batch(ctx)?;
    let time = load_smardex::block_secs(ctx, block).await?;
    let tag = call::block_tag(block);
    let mut calls = vec![
        ask(hook, &IAeroHookFee::secondsAgoCall {}),
        ask(hook, &IAeroHookFee::defaultScalingFactorCall {}),
        ask(hook, &IAeroHookFee::defaultFeeCapCall {}),
    ];
    for &(pool, spacing) in pools {
        calls.push(ask(pool, &IAeroClOracle::feeCall {}));
        calls.push(ask(pool, &IAeroClOracle::slot0Call {}));
        calls.push(ask(hook, &IAeroHookFee::dynamicFeeConfigCall { pool }));
        // tickSpacing пула — int24 (событие PoolCreated); не помещается — шаг 0 в пачке и отказ пулу ниже (firsts).
        let tick_spacing = I24::try_from(spacing).unwrap_or_default();
        let ask_fee = IAeroClTickFee::tickSpacingToFeeCall {
            tickSpacing: tick_spacing,
        };
        calls.push(ask(factory, &ask_fee));
    }
    let got = call::try_chunks(ctx, &calls, &tag, size).await;
    let defaults = match defaults(&got) {
        Ok(d) => d,
        Err(w) => {
            let why = format!("{}: крючок {hook}: {w}", f.id);
            return Err(ctx.err(ErrorCode::BadResponse, why));
        }
    };
    // Первый проход по пулам: (fee(), тик, index, cardinality, конфиг, tickSpacingToFee).
    let firsts: Vec<Result<First, String>> = pools
        .iter()
        .enumerate()
        .map(|(i, &(_, spacing))| {
            if I24::try_from(spacing).is_err() {
                return Err(format!("tickSpacing {spacing} не int24"));
            }
            first(&got[3 + i * FIRST..3 + (i + 1) * FIRST])
        })
        .collect();
    let deltas: Vec<u32> = (0..=ahead.min(defaults.seconds_ago)).collect();
    let mut second = Vec::new();
    for (&(pool, _), r) in pools.iter().zip(&firsts) {
        let Ok(x) = r else { continue };
        if x.cardinality == 0 {
            continue;
        }
        let next = U256::from((u32::from(x.index) + 1) % u32::from(x.cardinality));
        second.push(ask(pool, &IAeroClOracle::observationsCall { index: next }));
        let zero = U256::ZERO;
        second.push(ask(pool, &IAeroClOracle::observationsCall { index: zero }));
        for d in &deltas {
            let s = vec![defaults.seconds_ago - d, 0];
            second.push(ask(pool, &IAeroClOracle::observeCall { secondsAgos: s }));
        }
    }
    let got2 = call::try_chunks(ctx, &second, &tag, size).await;
    let mut at = 0usize;
    let mut out = Vec::with_capacity(pools.len());
    for r in firsts {
        let x = match r {
            Ok(x) => x,
            Err(why) => {
                out.push(Err(why));
                continue;
            }
        };
        let mut head = Head {
            time,
            tick: x.tick,
            cardinality: x.cardinality,
            oldest: Obs::default(),
            zero: Obs::default(),
            cums: Vec::new(),
        };
        if x.cardinality > 0 {
            let part = &got2[at..at + 2 + deltas.len()];
            at += 2 + deltas.len();
            match (obs(part.first()), obs(part.get(1))) {
                (Ok(o), Ok(z)) => (head.oldest, head.zero) = (o, z),
                (Err(w), _) | (_, Err(w)) => {
                    out.push(Err(w));
                    continue;
                }
            }
            // Откат observe — у крючка try/catch → 0 (L648-L660); сбой узла — пул не прочитан.
            let mut cums = Vec::with_capacity(deltas.len());
            let mut failed = None;
            for p in &part[2..] {
                match p {
                    Ok(Ok(_)) => match dec::<IAeroClOracle::observeCall>(Some(p)) {
                        Ok(r) => cums.push(pair(&r._0)),
                        Err(w) => failed = Some(w),
                    },
                    Ok(Err(_)) => cums.push(None),
                    Err(w) => failed = Some(format!("observe: узел {w}")),
                }
            }
            if let Some(w) = failed {
                out.push(Err(w));
                continue;
            }
            head.cums = cums;
        }
        let worst = ff::worst(&x.config, &defaults, x.ts_fee, &head, ahead);
        out.push(Ok(worst.max(x.view)));
    }
    Ok(out)
}

/// Первый проход по пулу.
struct First {
    view: u32,
    tick: i32,
    index: u16,
    cardinality: u16,
    config: Config,
    ts_fee: u32,
}

/// Общие параметры крючка из первых трёх ответов. Ошибки (текст): ответ не прочитан.
fn defaults(got: &[Piece]) -> Result<Defaults, String> {
    Ok(Defaults {
        seconds_ago: dec::<IAeroHookFee::secondsAgoCall>(got.first())?,
        scaling: dec::<IAeroHookFee::defaultScalingFactorCall>(got.get(1))?,
        fee_cap: dec::<IAeroHookFee::defaultFeeCapCall>(got.get(2))?,
    })
}

/// Разбор первого прохода пула (FIRST ответов). Ошибки (текст): ответ не прочитан.
fn first(p: &[Piece]) -> Result<First, String> {
    let view = dec::<IAeroClOracle::feeCall>(p.first())?;
    let s = dec::<IAeroClOracle::slot0Call>(p.get(1))?;
    let c = dec::<IAeroHookFee::dynamicFeeConfigCall>(p.get(2))?;
    let ts_fee = dec::<IAeroClTickFee::tickSpacingToFeeCall>(p.get(3))?;
    Ok(First {
        view: view.to::<u32>(),
        tick: s._1.as_i32(),
        index: s._2,
        cardinality: s._3,
        config: Config {
            base_fee: c._0.to::<u32>(),
            fee_cap: c._1.to::<u32>(),
            scaling: c._2,
            initial_enabled: c._3,
            initial_fee: c._4.to::<u32>(),
        },
        ts_fee: ts_fee.to::<u32>(),
    })
}

/// Наблюдение: время и признак инициализации. Ошибки (текст): ответ не прочитан.
fn obs(p: Option<&Piece>) -> Result<Obs, String> {
    let o = dec::<IAeroClOracle::observationsCall>(p)?;
    Ok(Obs {
        time: o._0,
        initialized: o._3,
    })
}

/// (cum старый, cum новый) из ответа observe([S − δ, 0]). Ошибок нет: не два значения — None.
fn pair(c: &[I56]) -> Option<(i64, i64)> {
    match c {
        [old, new] => Some((old.as_i64(), new.as_i64())),
        _ => None,
    }
}
