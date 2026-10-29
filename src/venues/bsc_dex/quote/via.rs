//! Файл: venues/bsc_dex/quote/via.rs
//! Что делает: путь через промежуточную монету (решение 52): покупка «USDT → промежуточная (пул P1) → монета (пул P2)»,
//! продажа «монета → промежуточная (P2) → USDT (P1)» — в одной транзакции. Собирает два хопа в порядке исполнения и
//! отдаёт их общему расчёту маршрута (quote/route_calc.rs): каждый шаг по реальной ликвидности своего пула (та же
//! математика, что у прямого режима), со своей комиссией, крючком и налогом монеты на её стороне. Двухшаговый путь —
//! частный случай графа глубины 2 (решение 80): множитель-mid здесь один, расчёт пути — общий.
//! Кто вызывает: quote/via_pools.rs (лучшая пара), quote/smart.rs (доля через промежуточную), exec/live.rs.
//! Пулы шагов, допуск промежуточной монеты и выбор лучшей пары — quote/via_pools.rs.
//! От чего зависит: quote/route_calc.rs, quote/path.rs, quote/select.rs, pools/registry.rs, pools/mids.rs.
//! Настройки: через вызываемые — см. их шапки.
//!
//! Промежуточная монета должна быть проверена и без налогов (покупки, продажи, перевода): налог промежуточной на
//! переходе между пулами в одной транзакции официально не описан — не гадаем, такую монету не используем. Монета с
//! налогом — только пулы V2 на каждом шаге (решение 39; research/flow/bsc_smart_router.md, раздел 4.1).

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Side, TokenTaxes};
use crate::gateway::errors::GatewayResult;

use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::Quoting;
use super::path::Path;
use super::route_calc::{self, Leg, keep};
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
        let leg = |p: &Ok2, (token_in, token_out), (dec_in, dec_out)| Leg {
            entry: p.0.clone(),
            usable: p.1.clone(),
            token_in,
            token_out,
            dec_in,
            dec_out,
        };
        // Хопы — в порядке исполнения стороны; налог монеты на её стороне (покупка — выход P2, продажа — вход P2).
        let (legs, tax_keep) = match self.side {
            Side::Buy => (
                vec![
                    leg(p1, (self.usdt, m.address), (self.usdt_dec, mid_dec)),
                    leg(p2, (m.address, self.coin), (mid_dec, self.coin_dec)),
                ],
                keep(self.taxes.buy_pct),
            ),
            Side::Sell => (
                vec![
                    leg(p2, (self.coin, m.address), (self.coin_dec, mid_dec)),
                    leg(p1, (m.address, self.usdt), (mid_dec, self.usdt_dec)),
                ],
                keep(self.taxes.sell_pct),
            ),
        };
        route_calc::eval(&self.q.ctx, &legs, self.side, qty, tax_keep)
    }
}
