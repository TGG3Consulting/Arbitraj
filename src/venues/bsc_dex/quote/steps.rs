//! Файл: venues/bsc_dex/quote/steps.rs
//! Что делает: затраты одного шага пути (решения 35, 52) — у каждого обмена своя строка с шагом маршрута (Cost::with_step):
//! газ обмена в этом пуле (BNB), комиссия пула (в токене входа шага), налог монеты на её шаге (покупка — шаг, где
//! монета на выходе; продажа — где на входе) — налог ИМЕННО пула этого шага (решение 146, quote/pool_tax.rs), влияние крючка (в токене выхода шага). Комиссия, налог и крючок уже
//! в цене (in_price = true — для прозрачности), газ — отдельно. Валюта каждой затраты — тикер, который ядро может
//! пересчитать: USDT (venue.quote_asset), тикер монеты, тикер промежуточной (биржи или «BNB» для WBNB).
//! Кто вызывает: quote/costs.rs.
//! От чего зависит: quote/path.rs, quote/pool_quote.rs (комиссия V4), quote/pool_tax.rs (налог пула шага), pools/mids.rs, num.rs, core/types (Cost, RouteStep).
//! Настройки: dex.v2_fee_pct [вид пула V2].
//!
//! Проскальзывания отдельным видом затраты нет (ответ техлида, Issue #7): цена каждого шага уже посчитана по реальной
//! ликвидности его пула (математика пула — в средней цене котировки), а exec.dex_slippage_pct — защита минимумом выхода
//! при исполнении, не затрата.
//! Шаг маршрута и валюты затрат — стык core/types/cost.rs (RouteStep, Cost::with_step) и ответ ядра в PR #25.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Cost, CostKind, PoolKind, PoolState, RouteStep, Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::{algebra_fee, crypto_swap, solidly_fee};
use super::super::num;
use super::super::pools::mids::Mids;
use super::super::pools::registry::Entry;
use super::path::{Hop, Path};
use super::pool_quote::swap_fee;
use super::pool_tax::step_pct;

/// Тикеры токенов пути для затрат.
pub struct Names<'a> {
    pub usdt: (Address, &'a str),
    pub coin: (Address, &'a str),
    pub mids: &'a Mids,
    /// Решение 416: тикер газа сети (dex.gas_ticker) — им пишется газ шага.
    pub gas: &'a str,
}

impl Names<'_> {
    /// Тикер токена. Ошибки: BadResponse — токен не USDT, не монета и не промежуточная.
    pub fn of(&self, ctx: &Ctx, a: Address) -> GatewayResult<String> {
        if a == self.usdt.0 {
            Ok(self.usdt.1.to_string())
        } else if a == self.coin.0 {
            Ok(self.coin.1.to_string())
        } else {
            self.mids.ticker(a).ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("тикер токена {a} неизвестен — затрату не пересчитать"),
                )
            })
        }
    }

    /// Шаг маршрута «вход → выход» с номером (с 1). Ошибки: как у of.
    pub fn step(
        &self,
        ctx: &Ctx,
        index: u32,
        from: Address,
        to: Address,
    ) -> GatewayResult<RouteStep> {
        Ok(RouteStep {
            index,
            from: self.of(ctx, from)?,
            to: self.of(ctx, to)?,
            from_token: Some(from),
            to_token: Some(to),
        })
    }
}

/// Затраты всех шагов пути. Принимает: путь, сторону, налоги монеты из проверки (запасные — когда монета сделки не
/// монета пула шага, quote/pool_tax.rs step_pct), цену газа (wei), тикеры.
/// Ошибки: BadResponse (числа, тикер), NotConfigured (dex.v2_fee_pct).
pub fn path_costs(
    ctx: &Ctx,
    p: &Path,
    side: Side,
    t: &TokenTaxes,
    gas_wei: U256,
    names: &Names<'_>,
) -> GatewayResult<Vec<Cost>> {
    let pct = |v: Decimal, x: Decimal| v * x / Decimal::ONE_HUNDRED;
    let coin = names.coin.0;
    let mut out = Vec::new();
    for (i, h) in p.hops.iter().enumerate() {
        let step = names.step(ctx, i as u32 + 1, h.token_in, h.token_out)?;
        let (from, to) = (step.from.clone(), step.to.clone());
        let add = |out: &mut Vec<Cost>, kind, amount, asset: &str, in_price| {
            out.push(Cost::new(kind, amount, asset, in_price).with_step(step.clone()));
        };
        add(
            &mut out,
            CostKind::Gas,
            gas_bnb(ctx, gas_wei, h)?,
            names.gas,
            false,
        );
        if let PoolState::Virtuals(state)=&h.entry.pool.state {
            let (raw,asset)=super::super::math::virtuals::charge(state,h.token_in,&h.swapped)
                .ok_or_else(||ctx.err(ErrorCode::BadResponse,"Virtuals: не удалось проверить налог на VIRTUAL"))?;
            let (decimals,symbol)=if h.token_in==asset{(h.dec_in,&from)}else{(h.dec_out,&to)};
            let value=num::to_decimal(raw,decimals).ok_or_else(||ctx.err(ErrorCode::BadResponse,"Virtuals: налог не помещается в Decimal"))?;
            add(&mut out,CostKind::PoolFee,value,symbol,true);
        } else if let PoolState::EtherVista(state) = &h.entry.pool.state {
            let fee = if h.token_in == state.wrapped { state.buy_fee } else { state.sell_fee };
            let amount = num::to_decimal(fee, num::BNB_DECIMALS)
                .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "EtherVista: плата в ETH не помещается в Decimal"))?;
            add(&mut out, CostKind::PoolFee, amount, names.gas, true);
        } else if let PoolState::Memebox(state)=&h.entry.pool.state {
            let fee=super::super::math::memebox::charged(state,h.token_in==h.entry.pool.token0,h.swapped.amount_in)
                .and_then(|raw|num::to_decimal(raw,h.dec_in))
                .ok_or_else(||ctx.err(ErrorCode::BadResponse,"Memebox: не подтверждена комиссия собственного входа"))?;
            add(&mut out,CostKind::PoolFee,fee,&from,true);
        } else {
            add(
                &mut out,
                CostKind::PoolFee,
                pct(h.pool_in, fee_for_input(ctx, &h.entry, h.token_in, h.swapped.amount_in)?),
                &from,
                true,
            );
        }
        if let PoolState::CandySwap(state) = &h.entry.pool.state {
            let fee=super::super::math::v2::fee_ratio(super::pool_quote::v2_fee_pct(ctx,&h.entry)?)
                .ok_or_else(||ctx.err(ErrorCode::BadResponse,"CandySwap: неверная комиссия пары"))?;
            let (_,next)=super::super::math::candy::funded(state,h.token_in==h.entry.pool.token0,fee,true,h.swapped.amount_in)
                .ok_or_else(||ctx.err(ErrorCode::NoRoute,"CandySwap: не восстановлен выход до удержания"))?;
            let output=usize::from(h.token_in==h.entry.pool.token0);
            let gross=state.balances[output]-next.balances[output];
            let cut=gross-super::super::math::candy::net(gross,state.payout_bps)
                .ok_or_else(||ctx.err(ErrorCode::BadResponse,"CandySwap: удержание не представимо"))?;
            let cost=num::to_decimal(cut,h.dec_out)
                .ok_or_else(||ctx.err(ErrorCode::BadResponse,"CandySwap: сумма комиссии не представима"))?;
            add(&mut out,CostKind::PoolFee,cost,&to,true);
        }
        if side == Side::Buy && h.token_out == coin {
            add(
                &mut out,
                CostKind::BuyTax,
                pct(p.coins, step_pct(&h.entry, &h.usable, coin, side, t)),
                &from_coin(names),
                true,
            );
        }
        if side == Side::Sell && h.token_in == coin {
            add(
                &mut out,
                CostKind::SellTax,
                pct(p.coins, step_pct(&h.entry, &h.usable, coin, side, t)),
                &from_coin(names),
                true,
            );
        }
        add(
            &mut out,
            CostKind::HookImpact,
            pct(h.pool_out, h.usable.hook_pct),
            &to,
            true,
        );
    }
    Ok(out)
}

/// Тикер монеты. Ошибок нет.
fn from_coin(names: &Names<'_>) -> String {
    names.coin.1.to_string()
}

/// Газ обмена в пуле шага, BNB. Ошибки: BadResponse.
fn gas_bnb(ctx: &Ctx, gas_wei: U256, h: &Hop) -> GatewayResult<Decimal> {
    num::to_decimal(
        gas_wei.saturating_mul(U256::from(h.usable.gas)),
        num::BNB_DECIMALS,
    )
    .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "газ не помещается в Decimal"))
}

/// Полная комиссия пула в процентах для направления «токен входа → другой». Ошибки: NotConfigured (V2), BadResponse.
pub fn fee_for_input(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    input: U256,
) -> GatewayResult<Decimal> {
    if let PoolState::DeltaSwap {
        reserve0,
        reserve1,
        fee,
    } = &e.pool.state
    {
        let (rin, rout) = if token_in == e.pool.token0 {
            (*reserve0, *reserve1)
        } else {
            (*reserve1, *reserve0)
        };
        let rate = super::super::math::deltaswap::rate(fee, rin, rout, input).ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                "DeltaSwap: объём не даёт торгуемую ликвидность",
            )
        })?;
        return Ok(Decimal::from(rate) / Decimal::TEN);
    }
    fee_pct(ctx, e, token_in)
}

pub fn fee_pct(ctx: &Ctx, e: &Entry, token_in: Address) -> GatewayResult<Decimal> {
    let pips = |p: u32| Decimal::from(p) / Decimal::from(10_000u32);
    let zero_for_one = token_in == e.pool.token0;
    match (&e.pool.state, e.pool.kind) {
        (PoolState::Virtuals(_) | PoolState::EtherVista(_), PoolKind::V2) => Ok(Decimal::ZERO),
        (_, PoolKind::V2) => super::pool_quote::v2_fee_pct(ctx, e),
        (_, PoolKind::V3) => e
            .pool
            .fee_pips
            .map(pips)
            .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "у пула V3 нет fee")),
        (
            PoolState::Cl {
                protocol_fee,
                lp_fee,
                ..
            }
            | PoolState::Bin {
                protocol_fee,
                lp_fee,
                ..
            },
            _,
        ) => Ok(pips(swap_fee(*protocol_fee, *lp_fee, zero_for_one))),
        // KyberSwap Elastic (решение 81): комиссия в feeUnits (FEE_UNITS = 100000 = 100%) → проценты = feeUnits / 1000.
        (PoolState::Elastic { fee_units, .. }, _) => {
            Ok(Decimal::from(*fee_units) / Decimal::from(1000u32))
        }
        // DODO PMM (решение 86b): комиссия пула = lpFeeRate + mtFeeRate (фикс-точка 18 знаков) — как querySell*
        // (DVMTrader.sol). Проценты = (lp + mt) / 1e18 · 100.
        (PoolState::Pmm(d), _) => {
            let rate = num::to_decimal(d.lp_fee_rate.saturating_add(d.mt_fee_rate), 18)
                .ok_or_else(|| {
                    ctx.err(
                        ErrorCode::BadResponse,
                        "комиссия DODO не помещается в Decimal",
                    )
                })?;
            Ok(rate * Decimal::ONE_HUNDRED)
        }
        // Solidly (Topaz v2, THENA V1; решения 80, 85): комиссия пула — числитель над 10000 (getFee) → проценты = fee/100
        // (одна комиссия на обе ветви stable/volatile; направление цены комиссию не меняет). Решение 273: по правилу
        // пула (math/solidly_fee.rs) — числитель над делителем; делитель (usdfi) → 100 / fee.
        // Решение 443: направленная комиссия (Camelot V2) — комиссия входа этого направления (fee1 — вход token1).
        (PoolState::Solidly { fee, fee1, .. }, _) => {
            let fee = solidly_fee::side(fee, fee1.as_ref(), zero_for_one);
            solidly_fee::pct(fee)
                .ok_or_else(|| ctx.err(ErrorCode::BadResponse, solidly_fee::no_pct(fee)))
        }
        // StableSwap (решение 122 F): комиссия — числитель над 10^10 (fee() пула) → проценты = fee / 10^8.
        // Решение 484: пул Wombat — haircutRate над 10^18 → проценты = haircut / 10^16 (комиссия за высокое покрытие —
        // только в расчёте обмена, math/wombat.rs: зависит от суммы).
        (PoolState::StableSwap(d), _) if d.wombat.is_some() => {
            let h = d.wombat.as_ref().map_or(U256::ZERO, |w| w.haircut);
            num::to_decimal(h, 16).ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    "haircut Wombat не помещается в Decimal",
                )
            })
        }
        (PoolState::StableSwap(d), _) => num::to_decimal(d.fee, 8).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "комиссия StableSwap не помещается в Decimal",
            )
        }),
        // Crypto-пул Curve (решение 442): комиссия при текущих балансах — `_fee(xp)` пула над 10^10 → проценты = fee /
        // 10^8 (math/crypto_swap.rs fee; при обмене котировка считает её на балансах после обмена).
        (PoolState::Crypto(d), _) => crypto_swap::scaled(d, &d.balances)
            .and_then(|xp| crypto_swap::fee(d, &xp))
            .and_then(|f| num::to_decimal(f, 8))
            .ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    "комиссия crypto-пула Curve не считается",
                )
            }),
        // TickBin (Maverick V2, решение 122 F): комиссия по направлению fee(tokenAIn), D18 (1e18 = 100%) → проценты =
        // fee / 1e16; tokenA = token0.
        (PoolState::TickBin(d), _) => {
            let fee = if zero_for_one { d.fee0_in } else { d.fee1_in };
            num::to_decimal(fee, 16).ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    "комиссия Maverick V2 не помещается в Decimal",
                )
            })
        }
        // V2 с усилением (KyberSwap Classic DMM, решение 287): комиссия блока над 10^18 → проценты = fee / 10^16.
        (PoolState::AmpV2(d), _) => num::to_decimal(d.fee, 16).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "комиссия KyberSwap DMM не помещается в Decimal",
            )
        }),
        // Пул в общем хранилище (Balancer V2, решение 289): комиссия обмена пула над 10^18 → проценты = fee / 10^16.
        (PoolState::VaultPool(d), _) => num::to_decimal(d.fee, 16).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "комиссия Balancer не помещается в Decimal",
            )
        }),
        // Пара с фиктивными резервами (SmarDex, решение 288): комиссии feesLP + feesPool над 10^6 → проценты = сумма / 10^4.
        (PoolState::Fictive(d), _) => num::to_decimal(d.fees_lp.saturating_add(d.fees_pool), 4)
            .ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    "комиссия SmarDex не помещается в Decimal",
                )
            }),
        // Бины с изменчивой комиссией (Liquidity Book V2.1, решение 290): полная комиссия активного бина на времени блока
        // чтения над 10^18 → проценты = fee / 10^16 (дальше по бинам изменчивая часть растёт — её считает котировка).
        (PoolState::LbBin(d), _) => super::super::math::lb_fee::active_fee(d)
            .and_then(|f| num::to_decimal(U256::from(f), 16))
            .ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    "комиссия Liquidity Book не считается",
                )
            }),
        // Ревью t189 (Н4, дефект до решения 404): Algebra (THENA Fusion, Algebra Integral; вид ядра Cl, состояние Algebra)
        // раньше падала в «состояние пула не того вида» — затраты маршрута и сигнал цикла с таким шагом не собирались.
        // Комиссия — та же, что берёт котировка (pool_quote.rs → math/swap_algebra.rs): globalState.fee в пипсах (1e-6);
        // решение 444: у направленных комиссий (Camelot V3) — комиссия стороны обмена (math/algebra_fee.rs).
        (PoolState::Algebra { fee, fee_otz, .. }, _) => {
            Ok(pips(algebra_fee::swap_fee(*fee, *fee_otz, zero_for_one)))
        }
        // PMM от оракула (WooFi, Metric, iZiSwap, Biswap V3): котировка — лесенка ответов квотера пула (pool_quote.rs,
        // quote/pmm_ladder.rs), квотер возвращает выход обмена — комиссия пула уже в нём; отдельной строки нет — 0
        // (строка затрат «в цене», in_price, — для прозрачности, в прибыль не вычитается).
        (PoolState::OraclePmm(_), _) => Ok(Decimal::ZERO),
        // Скважина Basin (решение 485): своей комиссии у скважины нет (Well.sol: выход задаёт только функция скважины).
        (PoolState::BasinWell(_), _) => Ok(Decimal::ZERO),
        // Пул Fluid DEX T1 (решение 440): комиссия обмена над 1e6 → проценты = fee / 1e4 (quote/fluid_depth.rs).
        (PoolState::SmartColDebt(d), _) => super::fluid_depth::fee_pct(d).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "комиссия Fluid не помещается в Decimal",
            )
        }),
        // Пул Shell Protocol Proteus (решение 486): вход и выход округляются на сумму / BASE_FEE — ≈ 2 · 100 / BASE_FEE %
        // (quote/shell_depth.rs); точная комиссия — в выходе котировки (math/proteus_swap.rs).
        (PoolState::Proteus(d), _) => super::shell_depth::fee_pct(d).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "BASE_FEE пула Shell не считается в проценты",
            )
        }),
        // Ekubo (решение 447): комиссия пула 0.64 (config, math/ekubo_config.rs) → проценты = fee · 100 / 2^64 (в 18
        // знаках); доплата MEVCapture — в выходе котировки (pool_quote.rs), отдельной строкой не идёт.
        (PoolState::Ekubo(d), _) => {
            let fee = U256::from(super::super::math::ekubo_config::fee(&d.config));
            let scaled = (fee * U256::from(10u8).pow(U256::from(20u8))) >> 64usize;
            num::to_decimal(scaled, 18)
                .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "комиссия Ekubo не считается"))
        }
        _ => Err(ctx.err(ErrorCode::BadResponse, "состояние пула не того вида")),
    }
}
