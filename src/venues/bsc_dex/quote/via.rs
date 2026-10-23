//! Файл: venues/bsc_dex/quote/via.rs
//! Что делает: путь через промежуточную монету (решение 52): покупка «USDT → промежуточная (пул P1) → монета (пул P2)»,
//! продажа «монета → промежуточная (P2) → USDT (P1)» — в одной транзакции. Каждый шаг считается по реальной
//! ликвидности своего пула (та же математика, что у прямого режима), со своей комиссией, крючком и налогом монеты на
//! её шаге. Покупка Q монет: P2 точный выход Q / (1 − налог покупки)(1 − крючок P2) → столько промежуточной нужно,
//! P1 точный выход этого / (1 − крючок P1) → столько USDT. Продажа: P2 точный вход Q·(1 − налог продажи) → промежуточная
//! после крючка P2 → P1 точный вход → USDT после крючка P1. Шаг, не исполнимый целиком в прочитанных пределах, — пары нет.
//! Кто вызывает: quote/via_pools.rs (лучшая пара), quote/smart.rs (доля через промежуточную), exec/live.rs.
//! Пулы шагов, допуск промежуточной монеты и выбор лучшей пары — quote/via_pools.rs.
//! От чего зависит: quote/path.rs, quote/select.rs, quote/pool_quote.rs, quote/direct.rs (ceil_units), check/state.rs,
//! pools/registry.rs, pools/mids.rs, num.rs.
//! Настройки: через вызываемые — см. их шапки.
//!
//! Промежуточная монета должна быть проверена и без налогов (покупки, продажи, перевода): налог промежуточной на
//! переходе между пулами в одной транзакции официально не описан — не гадаем, такую монету не используем. Монета с
//! налогом — только пулы V2 на каждом шаге (решение 39; research/flow/bsc_smart_router.md, раздел 4.1).

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::num;
use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::Quoting;
use super::direct::ceil_units;
use super::path::{Path, hop};
use super::pool_quote;
use super::select::Usable;

/// Что нужно для расчёта путей одной монеты.
pub struct Via<'a> {
    pub q: &'a Quoting,
    pub coin: Address,
    pub usdt: Address,
    pub side: Side,
    pub taxes: &'a TokenTaxes,
    pub coin_dec: u8,
    pub usdt_dec: u8,
}

/// Допущенный пул.
pub type Ok2 = (Entry, Usable);

impl Via<'_> {
    /// Путь через m в паре пулов. Принимает: промежуточную, её decimals, P1 («m — USDT»), P2 («монета — m»), количество.
    /// Ошибки: NoRoute (шаг не исполним целиком, налоги и крючки забирают всё), BadResponse (числа), NotConfigured.
    pub fn calc(
        &self,
        m: &Mid,
        mid_dec: u8,
        p1: &Ok2,
        p2: &Ok2,
        qty: Decimal,
    ) -> GatewayResult<Path> {
        let ctx = &self.q.ctx;
        let keep = |pct: Decimal| Decimal::ONE - pct / Decimal::ONE_HUNDRED;
        let bad = || {
            ctx.err(
                ErrorCode::BadResponse,
                "сумма не переводится в единицы токена",
            )
        };
        let partial = || {
            ctx.err(
                ErrorCode::NoRoute,
                "шаг через промежуточную не исполним целиком",
            )
        };
        let (k1, k2h) = (keep(p1.1.hook_pct), keep(p2.1.hook_pct));
        let (m_addr, decs1, decs2) = (
            m.address,
            (self.usdt_dec, mid_dec),
            (mid_dec, self.coin_dec),
        );
        let path = match self.side {
            Side::Buy => {
                let k2 = keep(self.taxes.buy_pct) * k2h;
                if k1 <= Decimal::ZERO || k2 <= Decimal::ZERO {
                    return Err(ctx.err(ErrorCode::NoRoute, "налог покупки и крючки забирают всё"));
                }
                let out2 = ceil_units(qty / k2, self.coin_dec).ok_or_else(bad)?;
                let s2 = pool_quote::swap(ctx, &p2.0, m_addr, false, out2)?;
                let need = num::to_decimal(s2.amount_in, mid_dec).ok_or_else(bad)? / k1;
                let s1 = pool_quote::swap(
                    ctx,
                    &p1.0,
                    self.usdt,
                    false,
                    ceil_units(need, mid_dec).ok_or_else(bad)?,
                )?;
                if !(s1.complete && s2.complete) {
                    return Err(partial());
                }
                let coins =
                    (num::to_decimal(s2.amount_out, self.coin_dec).ok_or_else(bad)? * k2).min(qty);
                let usdt = num::to_decimal(s1.amount_in, self.usdt_dec).ok_or_else(bad)?;
                let h1 = hop(
                    ctx,
                    p1.0.clone(),
                    p1.1.clone(),
                    s1,
                    (self.usdt, m_addr),
                    decs1,
                )?;
                let h2 = hop(
                    ctx,
                    p2.0.clone(),
                    p2.1.clone(),
                    s2,
                    (m_addr, self.coin),
                    decs2,
                )?;
                Path {
                    hops: vec![h1, h2],
                    coins,
                    usdt,
                }
            }
            Side::Sell => {
                let into2 = num::to_units(qty * keep(self.taxes.sell_pct), self.coin_dec)
                    .ok_or_else(bad)?;
                let s2 = pool_quote::swap(ctx, &p2.0, self.coin, true, into2)?;
                let got = num::to_decimal(s2.amount_out, mid_dec).ok_or_else(bad)? * k2h;
                let s1 = pool_quote::swap(
                    ctx,
                    &p1.0,
                    m_addr,
                    true,
                    num::to_units(got, mid_dec).ok_or_else(bad)?,
                )?;
                if !(s1.complete && s2.complete) {
                    return Err(partial());
                }
                let usdt = num::to_decimal(s1.amount_out, self.usdt_dec).ok_or_else(bad)? * k1;
                let h2 = hop(
                    ctx,
                    p2.0.clone(),
                    p2.1.clone(),
                    s2,
                    (self.coin, m_addr),
                    (self.coin_dec, mid_dec),
                )?;
                let h1 = hop(
                    ctx,
                    p1.0.clone(),
                    p1.1.clone(),
                    s1,
                    (m_addr, self.usdt),
                    (mid_dec, self.usdt_dec),
                )?;
                Path {
                    hops: vec![h2, h1],
                    coins: qty,
                    usdt,
                }
            }
        };
        Ok(path)
    }
}
