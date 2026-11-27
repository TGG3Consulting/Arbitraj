//! Файл: venues/bsc_dex/quote/pool_quote.rs
//! Что делает: обмен в одном пуле по его локальному состоянию — выбор математики по виду пула (V2, V3, CL, Elastic, Bin),
//! полная комиссия (V2 — настройка, V3 — fee пула, V4 — протокольная + LP по направлению), точный вход/выход.
//! Кто вызывает: quote/direct.rs, quote/liquidity.rs, quote/quoter.rs (сверка), check/hooks_probe.rs.
//! От чего зависит: math/*, pools/registry.rs, ctx.rs.
//! Настройки: dex.v2_fee_pct [DEX V2] — если у пары нет своей комиссии (dex.v2_fee_source = pool).
//!
//! Комиссия V4: протокольная по направлению — нижние 12 бит (zeroForOne) или верхние 12 (oneForZero);
//! swapFee = p + lp − p·lp/1e6 (p = 0 → lp) — ProtocolFeeLibrary.calculateSwapFee:
//! https://github.com/Uniswap/v4-core/blob/bea7f6d1c11cd5dc4f0d5151ffa37a1c239feee6/src/libraries/ProtocolFeeLibrary.sol#L17-L48 ,
//! infinity-core src/libraries/ProtocolFeeLibrary.sol#L51-L59; применение — Pool.sol#L300-L307, BinPool.sol#L124-L129.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::algebra_fee;
use super::super::math::swap_algebra::{self, AlgebraState, NextTick};
use super::super::math::swap_cl::{self, ClState, Swapped, Version};
use super::super::math::swap_dodo::{self, PmmState};
use super::super::math::swap_elastic::{self, ElasticState};
use super::super::math::{
    balancer_math, basin_well, bin_swap, crypto_swap, dmm, ekubo_mev, ekubo_swap, fluid_check,
    lb_swap, maverick_swap, smardex_swap, solidly_fee, solidly_v1_math, stable_ema, stable_guard,
    stableswap_ng, stableswap_nomi, stableswap_v1, swap_solidly, v2, wombat,
};
use super::super::pools::ekubo_native;
use super::super::pools::families::Proto;
use super::super::pools::family_variant::Variant;
use super::super::pools::load_dodo::{net_after_fees, rstate};
use super::super::pools::registry::Entry;

/// PIPS_DENOMINATOR = 1e6.
const PIPS: u64 = 1_000_000;

/// Обмен в пуле. Принимает: запись пула, какой токен отдаём, точный вход или выход, сумму в минимальных единицах.
/// Возвращает: Swapped. Ошибки: NotConfigured (dex.v2_fee_pct), NoRoute (нет карты/расчёт невозможен — как откат).
pub fn swap(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    exact_in: bool,
    amount: U256,
) -> GatewayResult<Swapped> {
    let zero_for_one = token_in == e.pool.token0;
    let why = |w: &str| {
        ctx.err(
            ErrorCode::NoRoute,
            format!(
                "{} {}: расчёт обмена невозможен: {w}",
                e.family.title, e.ticker
            ),
        )
    };
    let none = || {
        ctx.err(
            ErrorCode::NoRoute,
            format!(
                "{} {}: расчёт обмена невозможен (как откат контракта)",
                e.family.title, e.ticker
            ),
        )
    };
    match (&e.pool.state, e.pool.kind) {
        (PoolState::UmbraeDamm(s), PoolKind::V2) => super::super::math::umbrae_damm::fill(s, token_in, exact_in, amount)
            .map(|f| f.swapped).ok_or_else(none),
        (PoolState::Bundles(book),PoolKind::V2)=>book.swap(token_in,exact_in,amount).map(|(fill,_)|fill)
            .ok_or_else(||why("Bundles Swap: чистая выплата, минимальная комиссия или обеспечение недоступны")),
        (PoolState::Antfarm(book),PoolKind::V2) => book.swap(token_in,exact_in,amount)
            .ok_or_else(||why("Antfarm: объём или комиссия ATF не обеспечены по прочитанным рынкам")),
        (PoolState::Temple(s),PoolKind::V2) => {
            let out=if zero_for_one {e.pool.token1} else {e.pool.token0};
            super::super::math::temple::quote(s,token_in,out,exact_in,amount)
                .ok_or_else(||why("TempleDAO: объём, фактическая валюта выплаты или обеспечение не подходят для этой стороны"))
        }
        (PoolState::YieldBasisLev(s),PoolKind::V2)=>(if exact_in{s.swap(token_in,amount)}else{s.forward_for_output(token_in,amount)})
            .map(|f|Swapped{amount_in:f.spent,amount_out:f.received,complete:true}).ok_or_else(none),
        (PoolState::BalancerV1(s),PoolKind::V2)=>{
            let output=if zero_for_one{e.pool.token1}else{e.pool.token0};
            let fill=if exact_in{s.swap(token_in,output,amount,true,U256::MAX)}else{s.forward_for_output(token_in,output,amount)};
            fill.map(|f|Swapped{amount_in:f.spent,amount_out:f.received,complete:true}).ok_or_else(none)
        }
        (PoolState::BancorV3(s),PoolKind::V2)=>(if exact_in{s.swap(token_in,true,amount,false)}else{s.input_for_forward(token_in,amount,false)})
            .map(|f|Swapped{amount_in:f.spent,amount_out:f.received,complete:true}).ok_or_else(none),
        (PoolState::UniswapV1(s),PoolKind::V2)=>s.swap(token_in,exact_in,amount)
            .map(|f|Swapped{amount_in:f.spent,amount_out:f.received,complete:true}).ok_or_else(none),
        (PoolState::Rubicon(book),PoolKind::V2) => {
            let fill=book.plan(token_in,exact_in,amount).ok_or_else(none)?;
            Ok(Swapped{amount_in:fill.spent,amount_out:fill.received,complete:fill.complete})
        }
        (PoolState::Carbon(s),PoolKind::V2) => {
            let size=u128::try_from(amount).map_err(|_|why("Carbon: размер превышает uint128"))?;
            let p=super::super::math::carbon_match::plan(s,token_in,!exact_in,size).ok_or_else(none)?;
            Ok(Swapped{amount_in:U256::from(p.spent),amount_out:U256::from(p.received),complete:true})
        }
        (PoolState::Surge(s),PoolKind::V2)=>s.swap(token_in,exact_in,amount).ok_or_else(none),
        (PoolState::Dagrow(s),PoolKind::V2)=>s.fill(token_in,exact_in,amount).ok_or_else(none),
        (PoolState::MintClub(s), PoolKind::V2) => super::super::math::mint_club::fill(s,token_in,exact_in,amount).map(|f| f.swapped).ok_or_else(none),
        (PoolState::Virtuals(s), PoolKind::V2) => super::super::math::virtuals::quote(s,token_in,exact_in,amount).ok_or_else(none),
        (PoolState::Memebox(s), PoolKind::V2) => super::super::math::memebox::quote(s,zero_for_one,exact_in,amount)
            .map(|(fill,_)|fill).ok_or_else(none),
        (PoolState::CandySwap(s), PoolKind::V2) => {
            let fee=v2::fee_ratio(v2_fee_pct(ctx,e)?).ok_or_else(none)?;
            super::super::math::candy::funded(s,zero_for_one,fee,exact_in,amount)
                .map(|(fill,_)|fill).ok_or_else(none)
        }
        (PoolState::ToonSwap(state), PoolKind::V2) => state.trade(zero_for_one,exact_in,amount).map(|(s,_)|s).ok_or_else(none),
        (PoolState::EtherVista(state), PoolKind::V2) =>
            super::super::math::ethervista::quote(state, token_in, zero_for_one, exact_in, amount)
                .ok_or_else(|| why("EtherVista: объём не покрывает фиксированную плату или не помещается в резервы")) ,
        (
            PoolState::DeltaSwap {
                reserve0,
                reserve1,
                fee,
            },
            PoolKind::V2,
        ) => super::super::math::deltaswap::quote(
            *reserve0,
            *reserve1,
            fee,
            zero_for_one,
            exact_in,
            amount,
        )
        .ok_or_else(none),
        (PoolState::V2 { reserve0, reserve1 }, PoolKind::Cl)
            if e.family.variant == super::super::pools::family_variant::Variant::DeliConstant => {
            let reserves=if zero_for_one {(*reserve0,*reserve1)} else {(*reserve1,*reserve0)};
            super::super::math::deli::quote(reserves,e.pool.fee_pips.ok_or_else(none)?,exact_in,amount).ok_or_else(none)
        }
        (PoolState::V2 { reserve0, reserve1 } | PoolState::Oasis { reserve0, reserve1, .. }, PoolKind::V2) => {
            let fee = v2::fee_ratio(v2_fee_pct(ctx, e)?)
                .ok_or_else(|| ctx.not_set("dex.v2_fee_pct", "комиссия пары V2 вне 0–100%"))?;
            let (r_in, r_out) = if zero_for_one {
                (*reserve0, *reserve1)
            } else {
                (*reserve1, *reserve0)
            };
            let r = if exact_in {
                v2::amount_out(amount, r_in, r_out, fee).map(|o| Swapped {
                    amount_in: amount,
                    amount_out: o,
                    complete: true,
                })
            } else {
                v2::amount_in(amount, r_in, r_out, fee).map(|i| Swapped {
                    amount_in: i,
                    amount_out: amount,
                    complete: true,
                })
            };
            r.ok_or_else(none)
        }
        (
            PoolState::V3 {
                sqrt_price_x96,
                liquidity,
                tick,
            },
            PoolKind::V3,
        ) => {
            let fee = e.pool.fee_pips.ok_or_else(none)?;
            let ticks = e.ticks.as_ref().ok_or_else(none)?;
            let s = ClState {
                sqrt_price: *sqrt_price_x96,
                tick: *tick,
                liquidity: *liquidity,
                fee,
                ticks,
                version: Version::V3,
            };
            swap_cl::swap(&s, zero_for_one, exact_in, amount).map_err(why)
        }
        (
            PoolState::Cl {
                sqrt_price_x96,
                tick,
                liquidity,
                protocol_fee,
                lp_fee,
            },
            PoolKind::Cl,
        ) => {
            let ticks = e.ticks.as_ref().ok_or_else(none)?;
            let deli=e.family.variant==super::super::pools::family_variant::Variant::DeliCl;
            let fee = swap_fee(*protocol_fee, if deli {0} else {*lp_fee}, zero_for_one);
            let s = ClState {
                sqrt_price: *sqrt_price_x96,
                tick: *tick,
                liquidity: *liquidity,
                fee,
                ticks,
                version: Version::V4,
            };
            if deli {
                let wblt=ctx.dex_addr("dex.deli_fee_currency",e.family.id)?;
                return super::super::math::deli_cl::quote(&s,zero_for_one,token_in==wblt,*lp_fee,exact_in,amount).ok_or_else(none);
            }
            swap_cl::swap(&s, zero_for_one, exact_in, amount).map_err(why)
        }
        // KyberSwap Elastic (решение 81): L = baseL + reinvestL, своя математика (math/swap_elastic, 81a).
        (
            PoolState::Elastic {
                sqrt_price_x96,
                tick,
                base_l,
                reinvest_l,
                fee_units,
            },
            _,
        ) => {
            let ticks = e.ticks.as_ref().ok_or_else(none)?;
            let s = ElasticState {
                sqrt_p: *sqrt_price_x96,
                current_tick: *tick,
                base_l: *base_l,
                reinvest_l: *reinvest_l,
                fee_units: *fee_units,
                ticks,
                v1: e.family.variant == Variant::ElasticV1,
            };
            // is_token0 = направление задано в token0 (= zero_for_one).
            swap_elastic::swap(&s, zero_for_one, exact_in, amount)
                .map(|r| Swapped {
                    amount_in: r.amount_in,
                    amount_out: r.amount_out,
                    complete: r.complete,
                })
                .map_err(why)
        }
        // Algebra (THENA Fusion, решения 80, 81): одна ликвидность, динамическая комиссия (globalState.fee, постоянна
        // на весь своп) — своя математика (math/swap_algebra, фаза A). Вид ядра Cl. Решение 463: цель шага по коду пула
        // (algebra_next: THENA Fusion — строка сжатого битмапа, Camelot V3 — строка сырого, Integral — связный список).
        // Решение 444: у направленных комиссий (Camelot V3) — комиссия своей стороны (math/algebra_fee.rs).
        (
            PoolState::Algebra {
                sqrt_price_x96,
                tick,
                liquidity,
                fee,
                fee_otz,
            },
            _,
        ) => {
            let ticks = e.ticks.as_ref().ok_or_else(none)?;
            let s = AlgebraState {
                sqrt_price: *sqrt_price_x96,
                tick: *tick,
                liquidity: *liquidity,
                fee: algebra_fee::swap_fee(*fee, *fee_otz, zero_for_one),
                ticks,
                next: algebra_next(e.family.proto),
            };
            swap_algebra::swap(&s, zero_for_one, exact_in, amount)
                .map(|r| Swapped {
                    amount_in: r.amount_in,
                    amount_out: r.amount_out,
                    complete: r.complete,
                })
                .map_err(why)
        }
        (
            PoolState::Bin {
                active_id,
                protocol_fee,
                lp_fee,
            },
            PoolKind::Bin,
        ) => {
            let bins = e.bins.as_ref().ok_or_else(none)?;
            let fee = swap_fee(*protocol_fee, *lp_fee, zero_for_one);
            let amount: u128 = amount.try_into().map_err(|_| none())?;
            // Bin: X = currency0; swapForY = отдаём X (токен0).
            bin_swap::swap(bins, *active_id, fee, zero_for_one, exact_in, amount).ok_or_else(none)
        }
        // DODO PMM (решения 80, 86b): своя кривая (math/swap_dodo, 86a) + комиссии lp/mt (как querySellBase/Quote,
        // DVMTrader.sol). Только точный вход: обратной формулы PMM под точный выход нет.
        (PoolState::Pmm(d), PoolKind::Pmm) => {
            if !exact_in {
                return Err(why(
                    "DODO: точный выход не поддерживается (нет обратной формулы PMM)",
                ));
            }
            let state = PmmState {
                i: d.i,
                k: d.k,
                b: d.b,
                q: d.q,
                b0: d.b0,
                q0: d.q0,
                r: rstate(d.r).ok_or_else(|| why("R вне 0..=2"))?,
            };
            // Отдаём базовую монету → sellBase (получаем quote), иначе sellQuote. База — token0/token1 по ориентации.
            let base = if d.base_is_token0 {
                e.pool.token0
            } else {
                e.pool.token1
            };
            let gross = if token_in == base {
                swap_dodo::sell_base_token(&state, amount)
            } else {
                swap_dodo::sell_quote_token(&state, amount)
            };
            let (gross, _new_r) = gross.ok_or_else(none)?;
            let out = net_after_fees(gross, d.lp_fee_rate, d.mt_fee_rate).ok_or_else(none)?;
            Ok(Swapped {
                amount_in: amount,
                amount_out: out,
                complete: true,
            })
        }
        // Solidly (Topaz v2, THENA V1; решения 80, 85): stable → свой инвариант x³y+xy³ (math/swap_solidly, HEAD b3065d8);
        // volatile → обычная V2 x·y=k (math/v2). Комиссия пула — fee (числитель над 10000) из состояния: stable передаёт
        // его прямо в get_amount_out, volatile — дробью (10000−fee)/10000 в v2. Стабильная сторона — только точный вход
        // (обратной формулы get_y под точный выход нет); волатильная — обе стороны (V2).
        // Решение 483: Ramses Legacy (вариант Solidly V1) — выход getAmountOut пары на 1 меньше формулы (Pair.sol L457:
        // `_getAmountOut(...) - 1`; math/solidly_v1_math.rs variant_out / variant_want).
        // Решение 122 F: Solidly V1 (THENA V1) — своя стабильная математика Pair.sol (math/solidly_v1_math); точный вход
        // волатильного пула — формула контракта (вход минус floor(in·fee/10000), затем in·R/(R+in) — одинакова у THENA V1
        // и Velodrome), а не V2-дробь (она округляет вход иначе); точный выход волатильного — по-прежнему V2-обратная
        // (getAmountIn у контракта нет).
        (
            PoolState::Solidly {
                reserve0,
                reserve1,
                stable,
                decimals0,
                decimals1,
                fee,
                gate,
                fee1,
            },
            _,
        ) => {
            // Решение 443: пара Camelot V2 — своя математика (направленная комиссия, арифметика Solidity 0.5).
            if e.family.proto == Proto::CamelotV2 {
                let st = (*reserve0, *reserve1, *stable, *decimals0, *decimals1);
                let q = (zero_for_one, exact_in, amount);
                let r = super::camelot_quote::swap(st, fee, fee1.as_ref(), q);
                return r.map_err(|w| why(&w));
            }
            // Решение 273: пауза фабрики — обмен пары откатился бы (предел цены газа проверяет план сделки, exec/solidly_gate.rs).
            if let Some(w) = solidly_fee::blocked(gate, None) {
                return Err(why(&w));
            }
            // Решение 273: комиссия вычитается из входа по правилу пула (числитель/делитель) — дальше формулы контракта
            // с нулевой комиссией (вход уже чистый; при правиле «числитель над 10000» — те же числа, что до решения 273).
            let net = || solidly_fee::net_in(amount, fee).ok_or_else(none);
            let (r_in, r_out, dec_in, dec_out) = if zero_for_one {
                (*reserve0, *reserve1, *decimals0, *decimals1)
            } else {
                (*reserve1, *reserve0, *decimals1, *decimals0)
            };
            if *stable {
                if !exact_in {
                    return Err(why(
                        "Solidly stable: точный выход не поддерживается (нет обратной формулы кривой)",
                    ));
                }
                let out = if e.family.proto == Proto::SolidlyV1 {
                    solidly_v1_math::stable_amount_out(
                        net()?,
                        r_in,
                        r_out,
                        dec_in,
                        dec_out,
                        U256::ZERO,
                    )
                } else {
                    swap_solidly::get_amount_out(net()?, r_in, r_out, dec_in, dec_out, U256::ZERO)
                }
                .and_then(|o| solidly_v1_math::variant_out(e.family.variant, o))
                .ok_or_else(none)?;
                Ok(Swapped {
                    amount_in: amount,
                    amount_out: out,
                    complete: true,
                })
            } else {
                // Volatile: дробь после комиссии по правилу пула ((10000−fee)/10000 у числителя над 10000; (fee−1)/fee у
                // делителя) — только для обратной V2-формулы точного выхода.
                let ratio = solidly_fee::keep_ratio(fee).ok_or_else(none)?;
                if exact_in {
                    solidly_v1_math::volatile_amount_out(net()?, r_in, r_out, U256::ZERO)
                        .and_then(|o| solidly_v1_math::variant_out(e.family.variant, o))
                        .map(|o| Swapped {
                            amount_in: amount,
                            amount_out: o,
                            complete: true,
                        })
                        .ok_or_else(none)
                } else {
                    // Решение 483: у Ramses Legacy выход пары на 1 меньше формулы — формуле нужен выход + 1.
                    let want =
                        solidly_v1_math::variant_want(e.family.variant, amount).ok_or_else(none)?;
                    v2::amount_in(want, r_in, r_out, ratio)
                        .map(|i| Swapped {
                            amount_in: i,
                            amount_out: amount,
                            complete: true,
                        })
                        .ok_or_else(none)
                }
            }
        }
        // StableSwap (решение 122 F): выход как в exchange пула (math/stableswap_v1); только точный вход — обратной
        // формулы у пула нет (get_dx в шаблоне 2020 отсутствует).
        (PoolState::StableSwap(d), PoolKind::StableSwap) => {
            if !exact_in {
                return Err(why(
                    "StableSwap: точный выход не поддерживается (нет обратной формулы у пула)",
                ));
            }
            let (i, j) = if zero_for_one {
                (d.i0, d.i1)
            } else {
                (d.i1, d.i0)
            };
            if d.halted {
                return Err(why(
                    "StableSwap: пул на паузе или выключен (обмен откатился бы)",
                ));
            }
            // Решение 484: пул Wombat — выход PoolV2.swap (math/wombat.rs: CoreV2, haircut, высокое покрытие, проверки
            // addCash / removeCash / покрытия 1 %; пауза актива входа).
            if d.wombat.is_some() {
                let out =
                    wombat::swap_out(d, usize::from(i), usize::from(j), amount).ok_or_else(none)?;
                return Ok(Swapped {
                    amount_in: amount,
                    amount_out: out,
                    complete: true,
                });
            }
            // Шаблон пула (решение 122 F, 3): точность A, раунды, откат при несходимости — из состояния.
            let t = stableswap_v1::Template {
                amp_precision: d.amp_precision,
                max_rounds: d.max_rounds,
                strict: d.strict,
                pair_d: d.pair_d,
            };
            let (i, j) = (i as usize, j as usize);
            // Шаблон StableSwap-NG (Infinity Stable, решение «122 (Infinity Stable)»): динамическая комиссия и D по
            // формуле NG — выход _exchange_received крючка (math/stableswap_ng.rs).
            let out = match &d.ng {
                // Решение 284: шаблон «комиссия со входа» (Nomiswap Stable) — выход getAmountOut пары
                // (math/stableswap_nomi.rs).
                _ if d.input_fee => stableswap_nomi::amount_out(
                    i,
                    j,
                    amount,
                    &d.balances,
                    &d.rates,
                    d.amp,
                    d.fee,
                    d.amp_precision,
                    d.max_rounds,
                ),
                Some(ng) => stableswap_ng::exchange_received(
                    i,
                    j,
                    amount,
                    &d.balances,
                    &d.rates,
                    d.amp,
                    d.fee,
                    ng.offpeg_fee_multiplier,
                )
                .map(|r| r.0),
                None => stableswap_v1::exchange_out_t(
                    i,
                    j,
                    amount,
                    &d.balances,
                    &d.rates,
                    d.amp,
                    d.fee,
                    &t,
                ),
            }
            .ok_or_else(none)?;
            // Проверка цены пула после обмена против оракула (Lista, math/stable_guard.rs): не пройдёт — обмен откатится.
            if let Some(g) = &d.guard
                && !stable_guard::passes(i, j, amount, &d.balances, &d.rates, d.amp, d.fee, &t, g)
            {
                return Err(why(
                    "StableSwap: цена пула после обмена уйдёт от оракула дальше порога пула (обмен откатился бы)",
                ));
            }
            // Решение 472 (доделка): реализации Curve с оракулом цены (Plain2BasicEMA / Plain2ETHEMA) — save_p после
            // обмена (pack_prices: цена < 2^128), иначе обмен откатится (math/stable_ema.rs).
            if d.ema && stable_ema::passes(i, j, amount, &d.balances, &d.rates, d.amp, &t).is_none()
            {
                return Err(why(
                    "StableSwap: save_p пула после обмена откатил бы его (цена пула ≥ 2^128)",
                ));
            }
            Ok(Swapped {
                amount_in: amount,
                amount_out: out,
                complete: true,
            })
        }
        // PMM от оракула (WooFi, решение 122 F, 3): своей математики нет (развёрнутый код закрыт) — выход по лесенке
        // ответов квотера контракта на блоке (quote/pmm_ladder.rs): точно в точках, между ними — средняя цена линейно,
        // выше последней точки — неисполнимо. Только точный вход.
        (PoolState::YieldBasisPool(s),PoolKind::V2)=>{
            let points=s.points(token_in).ok_or_else(none)?;
            let (amount_in,amount_out)=super::pmm_ladder::wallet_amounts(points,exact_in,amount).ok_or_else(none)?;
            Ok(Swapped{amount_in,amount_out,complete:true})
        }
        (PoolState::OraclePmm(d), PoolKind::OraclePmm) if super::uses_wallet_quoter(e.family.variant) => {
            let points=if zero_for_one==d.base_is_token0 {&d.ladder_base_in} else {&d.ladder_quote_in};
            let (amount_in,amount_out)=super::pmm_ladder::wallet_amounts(points,exact_in,amount).ok_or_else(none)?;
            Ok(Swapped{amount_in,amount_out,complete:true})
        }
        (PoolState::OraclePmm(d), kind) if kind==PoolKind::OraclePmm || (kind==PoolKind::Cl && e.family.variant.launch()) => {
            if !exact_in {
                return Err(why(
                    "PMM от оракула (WooFi / Metric): точный выход по лесенке не поддерживается",
                ));
            }
            // zero_for_one — вход token0; продаём базовую, если она token0 и вход token0 (или наоборот).
            let points = if zero_for_one == d.base_is_token0 {
                &d.ladder_base_in
            } else {
                &d.ladder_quote_in
            };
            if points.is_empty() {
                return Err(why(
                    "лесенки цен пула нет (dex.pmm_ladder_usd не задана; пара не с USDT — решение 297, а при dex.pmm_ladder_counter = да — нет курса встречного токена к USDT, решение 306; пауза, цена оракула / провайдера непригодна, реализация не та или пул откатил квотер)",
                ));
            }
            let out = super::pmm_ladder::out(points, amount).ok_or_else(|| {
                why("объём больше последней точки лесенки цен пула — неисполнимо")
            })?;
            Ok(Swapped {
                amount_in: amount,
                amount_out: out,
                complete: true,
            })
        }
        // TickBin (Maverick V2, решение 122 F): порт расчёта команды Maverick (math/maverick_swap.rs) без предела тика —
        // как шаг MAVERICK исполнителя (tickLimit = int32 max/min, contracts/ArbExecutor.sol); tokenA = token0.
        (PoolState::TickBin(d), PoolKind::TickBin) => {
            let limit = if zero_for_one { i32::MAX } else { i32::MIN };
            maverick_swap::swap(d, amount, zero_for_one, !exact_in, limit)
                .map(|r| Swapped {
                    amount_in: r.amount_in,
                    amount_out: r.amount_out,
                    complete: r.complete,
                })
                .ok_or_else(none)
        }
        // V2 с усилением (KyberSwap Classic DMM, решение 287): формула DMMLibrary по виртуальным резервам и комиссии блока
        // (math/dmm.rs); выход ограничен реальным резервом (иначе контракт откатит).
        (PoolState::AmpV2(d), PoolKind::AmpV2) => {
            let s = if zero_for_one {
                dmm::Side {
                    r_in: d.reserve0,
                    r_out: d.reserve1,
                    v_in: d.v_reserve0,
                    v_out: d.v_reserve1,
                }
            } else {
                dmm::Side {
                    r_in: d.reserve1,
                    r_out: d.reserve0,
                    v_in: d.v_reserve1,
                    v_out: d.v_reserve0,
                }
            };
            let r = if exact_in {
                dmm::amount_out(amount, s, d.fee).map(|o| (amount, o))
            } else {
                dmm::amount_in(amount, s, d.fee).map(|i| (i, amount))
            };
            r.map(|(amount_in, amount_out)| Swapped {
                amount_in,
                amount_out,
                complete: true,
            })
            .ok_or_else(none)
        }
        // Пара с фиктивными резервами (SmarDex, решение 288): порт SmardexLibrary.getAmountOut со средней ценой, приведённой
        // к времени блока чтения (math/smardex_swap.rs swap_out). Только точный вход (обратной формулы у пары нет в обмене по входу).
        (PoolState::Fictive(d), PoolKind::Fictive) => {
            if !exact_in {
                return Err(why("SmarDex: точный выход не поддерживается"));
            }
            let p = smardex_swap::side(d, zero_for_one);
            let out = smardex_swap::swap_out_version(
                &p,
                d.price_average_ts,
                d.block_ts,
                amount,
                e.family.variant == Variant::SmardexExactEquality,
            )
            .ok_or_else(none)?;
            Ok(Swapped {
                amount_in: amount,
                amount_out: out,
                complete: true,
            })
        }
        // Пул в общем хранилище (Balancer V2, решение 289): формула пула как по вызову хранилища (math/balancer_math.rs):
        // комиссия, множители точности, весовая / стейблсвап; индексы пары — из состояния (i0 — token0).
        (PoolState::VaultPool(d), PoolKind::VaultPool) => {
            let (i, j) = if zero_for_one {
                (d.i0, d.i1)
            } else {
                (d.i1, d.i0)
            };
            let r = if exact_in {
                balancer_math::amount_out(d, i.into(), j.into(), amount).map(|o| (amount, o))
            } else {
                balancer_math::for_output(d, i.into(), j.into(), amount)
            };
            r.map(|(amount_in, amount_out)| Swapped {
                amount_in,
                amount_out,
                complete: true,
            })
            .ok_or_else(none)
        }
        // Crypto-пул Curve (решение 442): выход как в `_exchange` пула (math/crypto_swap.rs: get_y Twocrypto / Tricrypto,
        // комиссия с выхода); только точный вход (шаг исполнителя — exchange на точный вход). Рампа A/gamma — D через
        // newton_D (решение 453); halted — обмены пула остановлены.
        (PoolState::Crypto(d), PoolKind::Crypto) => {
            if !exact_in {
                return Err(why("crypto-пул Curve: точный выход не поддерживается"));
            }
            if d.halted {
                return Err(why("crypto-пул Curve: обмены остановлены (is_killed)"));
            }
            let (i, j) = if zero_for_one {
                (d.i0, d.i1)
            } else {
                (d.i1, d.i0)
            };
            let Some(out) = crypto_swap::exchange_out(d, i.into(), j.into(), amount) else {
                // Решение 472: выход считается, а tweak_price после обмена откатил бы его (math/crypto_tweak2.rs).
                let mut bare = (**d).clone();
                bare.tweak = None;
                if d.tweak.is_some()
                    && crypto_swap::exchange_out(&bare, i.into(), j.into(), amount).is_some()
                {
                    return Err(why(
                        "crypto-пул Curve: tweak_price после обмена откатил бы его",
                    ));
                }
                return Err(none());
            };
            Ok(Swapped {
                amount_in: amount,
                amount_out: out,
                complete: true,
            })
        }
        // Бины с изменчивой комиссией (Liquidity Book V2.1, решение 290): порт getSwapOut по прочитанной полосе бинов на
        // времени блока чтения (math/lb_swap.rs). Только точный вход (шаг исполнителя — вход переводом и swap).
        (PoolState::LbBin(d), PoolKind::LbBin) => {
            if !exact_in {
                return Err(why("Liquidity Book: точный выход не поддерживается"));
            }
            let bins = e.bins.as_ref().ok_or_else(none)?;
            let amount: u128 = amount.try_into().map_err(|_| none())?;
            // X — token0 записи (x_is_token0) или token1; swapForY — отдаём X.
            let for_y = zero_for_one == d.x_is_token0;
            // Решение 446 (ревью С1): обмен через пул исключён (крючок обмена не допущен) — причина наружу.
            if let Some(blocked) = &d.swap_blocked {
                return Err(why(&format!("Liquidity Book: {blocked}")));
            }
            // Решение 446: LB V2.2 — ещё откат по ликвидности бина (math/lb_cap.rs).
            let swap = if e.family.proto == Proto::LbV22 {
                lb_swap::swap_out_v22
            } else {
                lb_swap::swap_out
            };
            let r = swap(d, bins, d.block_ts, for_y, amount).ok_or_else(none)?;
            Ok(Swapped {
                amount_in: U256::from(amount - r.in_left),
                amount_out: U256::from(r.out),
                complete: r.complete,
            })
        }
        // Пул Fluid DEX T1 (решение 440): порт _swapIn пула и проверки пределов слоя ликвидности и скачка цены
        // (math/fluid_swap.rs, fluid_check.rs); направление — по token0 самого пула. Только точный вход.
        // Скважина Basin (решение 485): swapFrom — резерв_j − calcReserve функции скважины (math/basin_well.rs); своей
        // комиссии нет. Только точный вход (шаг — swapFrom).
        (PoolState::BasinWell(d), PoolKind::BasinWell) => {
            if !exact_in {
                return Err(why("Basin: точный выход не поддерживается"));
            }
            if !d.has(token_in) {
                return Err(why("Basin: токен входа не из скважины"));
            }
            let out = basin_well::swap_out(d, token_in, amount).ok_or_else(none)?;
            Ok(Swapped {
                amount_in: amount,
                amount_out: out,
                complete: true,
            })
        }
        (PoolState::SmartColDebt(d), PoolKind::SmartColDebt) => {
            if !exact_in {
                return Err(why("Fluid: точный выход не поддерживается"));
            }
            if !d.has(token_in) {
                return Err(why("Fluid: токен входа не из пула"));
            }
            let q = fluid_check::quote_in(d, token_in == d.token0, amount);
            let out = q.ok_or_else(none)?;
            Ok(Swapped {
                amount_in: amount,
                amount_out: out,
                complete: true,
            })
        }
        // Пул Shell Protocol Proteus (решение 486): порт swapGivenInputAmount реализации и пути учёта Ocean (вход в
        // единицы учёта, выход — unwrap с комиссией вывода и знаками токена; math/proteus_swap.rs). Только точный вход.
        (PoolState::Proteus(d), PoolKind::Proteus) => {
            if !exact_in {
                return Err(why("Shell: точный выход не поддерживается"));
            }
            let in_x = d
                .is_x(token_in)
                .ok_or_else(|| why("Shell: токен входа не из пула"))?;
            let out =
                super::super::math::proteus_swap::quote_in(d, in_x, amount).ok_or_else(none)?;
            Ok(Swapped {
                amount_in: amount,
                amount_out: out,
                complete: true,
            })
        }
        // Ekubo (решение 447): порт swap ядра по прочитанному состоянию (math/ekubo_swap.rs), у пула с MEVCapture —
        // доплата с выхода (math/ekubo_mev.rs, худший tickLast). Только точный вход (шаг EKUBO меняет точный вход).
        // Решение 454: направление — в порядке ключа (нативный ETH — token0 ключа, pools/ekubo_native.rs); у пула с
        // TWAMM — состояние после виртуальных ордеров к двум возможным временам блока сделки, расчёт по худшему
        // (quote/ekubo_trade_state.rs).
        (PoolState::Ekubo(d), PoolKind::Ekubo) => {
            if !exact_in {
                return Err(why("Ekubo: точный выход не поддерживается"));
            }
            let amount: u128 = amount.try_into().map_err(|_| none())?;
            let is_token1 = ekubo_native::is_token1(d, e.pool.token0, token_in);
            let mut best: Option<Swapped> = None;
            let states = super::ekubo_trade_state::at_trade(d);
            for s in states.map_err(|w| why(&w))? {
                let r = ekubo_swap::exact_in(&s, is_token1, amount);
                let r = r.ok_or_else(none)?;
                let out = match &s.mev {
                    Some(m) => ekubo_mev::out_after(&s, m, s.tick, r.tick_after, r.amount_out),
                    None => r.amount_out,
                };
                let one = Swapped {
                    amount_in: U256::from(r.amount_in),
                    amount_out: U256::from(out),
                    complete: r.complete,
                };
                if best.as_ref().is_none_or(|b| one.amount_out < b.amount_out) {
                    best = Some(one);
                }
            }
            best.ok_or_else(none)
        }
        _ => Err(none()),
    }
}

/// Решения 461, 463: цель шага цикла обмена Algebra по коду пула (math/swap_algebra.rs, шапка, пункт 5): Algebra
/// Integral (связный список тиков) — Linked; THENA Fusion (Proto::Algebra, код AlgebraV1, сжатый битмап) и Algebra
/// V1.9 Camelot V3 (Proto::AlgebraDirFee, сырой битмап — шаг карты 1) — Row.
/// Принимает: протокол вида. Возвращает: режим. Ошибок нет.
pub fn algebra_next(proto: Proto) -> NextTick {
    if proto == Proto::AlgebraIntegral {
        NextTick::Linked
    } else {
        NextTick::Row
    }
}

/// Полная комиссия V4 по направлению (пипсы). Ошибок нет.
pub fn swap_fee(protocol_fee: u32, lp_fee: u32, zero_for_one: bool) -> u32 {
    let p = u64::from(if zero_for_one {
        protocol_fee & 0xfff
    } else {
        protocol_fee >> 12
    });
    let lp = u64::from(lp_fee & 0xff_ffff);
    if p == 0 {
        return lp as u32;
    }
    (p + lp - p * lp / PIPS) as u32
}

/// Комиссия пары V2 в процентах: своя у пары (fee_pips — прочитана с пары при проверке, dex.v2_fee_source = pool)
/// или общая для DEX (dex.v2_fee_pct). Ошибки: NotConfigured (dex.v2_fee_pct).
pub fn v2_fee_pct(ctx: &Ctx, e: &Entry) -> GatewayResult<Decimal> {
    match e.pool.fee_pips {
        Some(p) => Ok(Decimal::from(p) / Decimal::from(10_000u32)),
        None => ctx.dex_dec("dex.v2_fee_pct", e.family.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Как ProtocolFeeLibrary.calculateSwapFee: 0,1% протокольной и 0,3% LP → 0,3997%.
    #[test]
    fn fee_composition() {
        assert_eq!(swap_fee(1000, 3000, true), 1000 + 3000 - 3);
        assert_eq!(swap_fee(1000 << 12, 3000, true), 3000);
        assert_eq!(swap_fee(1000 << 12, 3000, false), 3997);
    }
}
