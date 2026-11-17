//! Файл: venues/bsc_dex/quote/cycle_rate.rs
//! Что делает: решение 404 — курс ребра графа для отбора лучших циклов DEX ↔ DEX (quote/cycle_rank.rs) и память одного
//! прохода (Rates). Курс ребра — минимальных единиц выхода за единицу входа на малом входе, с комиссией пула:
//! - по умолчанию (V2, Bin, LB, TickBin, DODO, PMM, AmpV2, Fictive, Vault, Solidly волатильный) — спот-цена пула по
//!   состоянию снимка (quote/spot.rs coins_for) × (1 − комиссия пула на этом направлении, quote/steps.rs fee_pct);
//! - Solidly стабильный (ревью t189, Н1) — производная кривой x³y + xy³ в текущей точке (math/solidly_math.rs d — порт
//!   Pool._d) × (1 − комиссия): отношение резервов у стабильной кривой неверно (перекос 1,5 : 1 — цена ≈ 1, а не 1,5);
//!   производная точна и для пар с decimals 18/6 и крошечными резервами (у пробы малой суммы там выход округлялся в 0 —
//!   прогон 70f97160, magicfox 18/6: резерв 0,68 USDT);
//! - StableSwap (Н2) и пулы сосредоточенной ликвидности V3 / CL (V4, Infinity) / Algebra / Elastic (прогон 70f97160:
//!   SquadSwap V3 CAKE/WBNB — цена slot0 0,0027119 стоит на границе тика, ниже — пустой участок около 2200 тиков; любой
//!   вход в сторону token0 → token1 исполняется по ≈ 0,00218, квотер контракта на 1 $ и 100 $ — 0,0021799, спот лгал на
//!   24 %) — котировка малой суммы той же математикой, что у сделки (quote/pool_quote.rs swap, комиссия внутри): вход =
//!   баланс входа (StableSwap) или виртуальный резерв входа в текущем диапазоне (L/√P или L·√P) / vault_depth::PROBE_DIV —
//!   та же доля «малой пробы цены», что у Balancer (spot.rs) и поиска глубины стейблов (решение 304), свойство алгоритма
//!   оценки, не порог торговли; выход меньше PROBE_DIV минимальных единиц — точность хуже той же доли — курса нет (пул
//!   мал для пробы; L = 0 в текущем диапазоне — тоже нет).
//! Курс во всех случаях — безразмерное «минимальных единиц выхода за минимальную единицу входа»: по замкнутому циклу
//! decimals сокращаются.
//!
//! Память прохода (своя у прохода, без замков, общая для всех базовых): курс ребра-стороны (Vec по индексу ребра·2 +
//! «вход — монета пула»); комиссия V2 без fee_pips — по виду DEX (dex.v2_fee_pct читается раз на вид за проход); стороны
//! пулов по книге глубины — ОДНИМ захватом замка чтения на проход (pools/depth_book.rs sides_many): глубина стороны (USD)
//! и «глубокая ли» (глубина в полосе ≥ dex.min_pool_liquidity_usd с гистерезисом, решение 73).
//! Кто вызывает: quote/cycle_rank.rs; тесты (edge_rate).
//! От чего зависит: quote/{spot, steps, pool_quote, vault_depth, bound_rate}.rs, math/{full_math, solidly_math}.rs,
//! pools/{graph, depth_book}.rs, num.rs,
//! core/types (PoolKind, PoolState).
//! Настройки: через steps / pool_quote — dex.v2_fee_pct [вид пула V2].

use std::collections::HashMap;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, PoolState};

use super::super::ctx::Ctx;
use super::super::math::full_math::mul_div;
use super::super::math::solidly_math;
use super::super::num;
use super::super::pools::depth_book::Book;
use super::super::pools::graph::{Edge, Graph};
use super::bound_rate::{Ratio, ceil_dec};
use super::{pool_quote, spot, steps, vault_depth};

/// Масштаб точности деления цены (знаков): сумма встречного токена 10^27 минимальных единиц в coins_for — точность
/// арифметики (как 2^96 в spot.rs), а не параметр торговли: курс считается в отношении минимальных единиц.
const SCALE_DIGITS: u8 = 27;

/// Сторона пула по книге глубины: глубина (USD) и глубокая ли.
pub type Side = Option<(Decimal, bool)>;

/// Память одного прохода (см. шапку).
pub struct Rates {
    rate: Vec<Option<Option<Decimal>>>,
    side: Vec<Side>,
    v2_fee: HashMap<&'static str, Option<Decimal>>,
}

impl Rates {
    /// Память на граф: стороны всех рёбер — одним чтением книги глубины. Принимает: граф, книгу. Ошибок нет.
    pub fn new(g: &Graph, book: &Book) -> Rates {
        let n = g.len();
        let all = book.sides_many((0..n).map(|i| &g.edge(i).entry.pool.reference));
        let mut side = vec![None; n.saturating_mul(2)];
        for (i, s) in all.into_iter().enumerate() {
            if let Some((buy, deep_buy, sell, deep_sell)) = s {
                // Индекс i·2 + 0 — вход встречный токен (покупка монеты пула), + 1 — вход монета (продажа).
                side[i * 2] = Some((buy, deep_buy));
                side[i * 2 + 1] = Some((sell, deep_sell));
            }
        }
        Rates {
            rate: vec![None; n.saturating_mul(2)],
            side,
            v2_fee: HashMap::new(),
        }
    }

    /// Курс ребра i при входе token_in (из памяти или посчитан один раз). None — курса нет. Ошибок нет.
    pub fn get(&mut self, ctx: &Ctx, g: &Graph, i: usize, token_in: Address) -> Option<Decimal> {
        let e = g.edge(i);
        let slot = slot(e, i, token_in);
        if let Some(r) = self.rate[slot] {
            return r;
        }
        let r = match by_quote(ctx, e, token_in) {
            Some(r) => r,
            None => self
                .fee(ctx, e, token_in)
                .and_then(|f| rate_with_fee(e, token_in, f)),
        };
        self.rate[slot] = Some(r);
        r
    }

    /// Сторона пула ребра i при входе token_in по книге (снимок прохода). Ошибок нет.
    pub fn side(&self, g: &Graph, i: usize, token_in: Address) -> Side {
        self.side[slot(g.edge(i), i, token_in)]
    }

    /// Комиссия пула, % — как steps.rs fee_pct; у V2 без fee_pips — dex.v2_fee_pct вида DEX из памяти прохода
    /// (как pool_quote.rs v2_fee_pct). None — комиссии нет. Ошибок нет.
    fn fee(&mut self, ctx: &Ctx, e: &Edge, token_in: Address) -> Option<Decimal> {
        if e.entry.pool.kind != PoolKind::V2 {
            return steps::fee_pct(ctx, &e.entry, token_in).ok();
        }
        if let Some(p) = e.entry.pool.fee_pips {
            return Some(Decimal::from(p) / Decimal::from(10_000u32));
        }
        let id = e.entry.family.id;
        *self
            .v2_fee
            .entry(id)
            .or_insert_with(|| ctx.dex_dec("dex.v2_fee_pct", id).ok())
    }
}

/// Индекс ребра-стороны: i·2 + (вход — монета пула). Ошибок нет.
fn slot(e: &Edge, i: usize, token_in: Address) -> usize {
    i * 2 + usize::from(token_in == e.entry.coin)
}

/// Курс ребра (тот же расчёт, что у отбора; отбор берёт комиссию V2 из памяти прохода). Принимает: окружение, ребро,
/// токен входа. Возвращает: курс; None — нет цены, нет комиссии, котировка малой суммы не считается. Ошибок нет.
pub fn edge_rate(ctx: &Ctx, e: &Edge, token_in: Address) -> Option<Decimal> {
    if let Some(r) = by_quote(ctx, e, token_in) {
        return r;
    }
    let fee = steps::fee_pct(ctx, &e.entry, token_in).ok()?;
    rate_with_fee(e, token_in, fee)
}

/// Курс не по споту (см. шапку): Solidly стабильный — производная кривой; StableSwap и пулы сосредоточенной ликвидности
/// (V3, CL V4 / Infinity, Algebra, Elastic) — котировка малой суммы. Возвращает: None — вид пула не этот (курс — по
/// споту); Some(None) — у этого вида курса нет (пустой пул, обмен не считается, пул мал для пробы); Some(Some(курс)).
fn by_quote(ctx: &Ctx, e: &Edge, token_in: Address) -> Option<Option<Decimal>> {
    let zfo = token_in == e.entry.pool.token0;
    let virtual_in = |sqrt: U256, l: u128| {
        // Виртуальный резерв токена входа в текущем диапазоне цены: token0 — L·2^96/√P, token1 — L·√P/2^96
        // (Uniswap v3 whitepaper, 6.2: x = L/√P, y = L·√P).
        let (q96, l) = (U256::from(1u8) << 96, U256::from(l));
        if sqrt.is_zero() {
            None
        } else if zfo {
            mul_div(l, q96, sqrt)
        } else {
            mul_div(l, sqrt, q96)
        }
    };
    let bal_in = match &e.entry.pool.state {
        PoolState::Solidly {
            reserve0,
            reserve1,
            stable: true,
            decimals0,
            decimals1,
            ..
        } => {
            let r = if zfo {
                (*reserve0, *reserve1, *decimals0, *decimals1)
            } else {
                (*reserve1, *reserve0, *decimals1, *decimals0)
            };
            return Some(solidly_stable(ctx, e, token_in, r));
        }
        PoolState::StableSwap(d) => {
            let i = if zfo { d.i0 } else { d.i1 };
            d.balances.get(usize::from(i)).copied()
        }
        PoolState::V3 {
            sqrt_price_x96,
            liquidity,
            ..
        }
        | PoolState::Cl {
            sqrt_price_x96,
            liquidity,
            ..
        }
        | PoolState::Algebra {
            sqrt_price_x96,
            liquidity,
            ..
        } => virtual_in(*sqrt_price_x96, *liquidity),
        PoolState::Elastic {
            sqrt_price_x96,
            base_l,
            reinvest_l,
            ..
        } => virtual_in(*sqrt_price_x96, base_l.saturating_add(*reinvest_l)),
        _ => return None,
    };
    Some(bal_in.and_then(|b| quote_rate(ctx, e, token_in, b)))
}

/// Курс котировкой малой суммы: вход = баланс (резерв) входа / PROBE_DIV, выход — pool_quote::swap (комиссия внутри).
/// Выход меньше PROBE_DIV минимальных единиц — точность хуже 1/PROBE_DIV (как и сдвиг цены пробой) — курса нет (пул мал
/// для пробы). Ошибок нет: None — пусто, обмен не считается или не исполнен целиком.
fn quote_rate(ctx: &Ctx, e: &Edge, token_in: Address, bal_in: U256) -> Option<Decimal> {
    let div = U256::from(vault_depth::PROBE_DIV);
    let probe = bal_in / div;
    if probe.is_zero() {
        return None;
    }
    let out = match pool_quote::swap(ctx, &e.entry, token_in, true, probe) {
        Ok(s) if s.complete => s.amount_out,
        _ => return None,
    };
    if out < div {
        return None;
    }
    ceil_dec(Ratio {
        num: out,
        den: probe,
    })
}

/// Курс Solidly стабильного пула — производная кривой x³y + xy³ в текущей точке (минимальных единиц выхода за единицу
/// входа) × (1 − комиссия): x, y — резервы, нормализованные к 1e18 по decimals (как в Pool.sol); dy/dx = (3x²y + y³) /
/// (x³ + 3xy²) = _d(y, x) / _d(x, y) (math/solidly_math.rs d — порт Pool._d velodrome-finance), в минимальных единицах — ×
/// dec_out / dec_in. Без округления пробы: верно и для пары с decimals 18/6 и крошечными резервами. Принимает: окружение
/// (комиссия), ребро, токен входа, (резерв входа, резерв выхода, 10^dec входа, 10^dec выхода). Ошибок нет: None — пустой
/// пул, переполнение, нет комиссии.
fn solidly_stable(
    ctx: &Ctx,
    e: &Edge,
    token_in: Address,
    (r_in, r_out, dec_in, dec_out): (U256, U256, U256, U256),
) -> Option<Decimal> {
    let one = solidly_math::e18();
    let x = mul_div(r_in, one, dec_in)?;
    let y = mul_div(r_out, one, dec_out)?;
    let num = solidly_math::d(y, x)?.checked_mul(dec_out)?;
    let den = solidly_math::d(x, y)?.checked_mul(dec_in)?;
    if num.is_zero() || den.is_zero() {
        return None;
    }
    let fee = steps::fee_pct(ctx, &e.entry, token_in).ok()?;
    let keep = Decimal::ONE - fee / Decimal::ONE_HUNDRED;
    if keep <= Decimal::ZERO {
        return None;
    }
    ceil_dec(Ratio { num, den })?.checked_mul(keep)
}

/// Курс ребра по спот-цене при известной комиссии пула (%). Ошибок нет: None — нет цены, переполнение, комиссия ≥ 100 %.
fn rate_with_fee(e: &Edge, token_in: Address, fee: Decimal) -> Option<Decimal> {
    let one = U256::from(10u8).pow(U256::from(SCALE_DIGITS));
    let coins = spot::coins_for(&e.entry, one)?;
    let per = num::to_decimal(coins, SCALE_DIGITS)?;
    if per <= Decimal::ZERO {
        return None;
    }
    let raw = if token_in == e.entry.coin {
        Decimal::ONE.checked_div(per)?
    } else {
        per
    };
    let keep = Decimal::ONE - fee / Decimal::ONE_HUNDRED;
    if keep <= Decimal::ZERO {
        return None;
    }
    raw.checked_mul(keep)
}
