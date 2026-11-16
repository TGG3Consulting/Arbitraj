//! Файл: venues/bsc_dex/quote/via.rs
//! Что делает: путь через промежуточную монету (решение 52): покупка «USDT → промежуточная (пул P1) → монета (пул P2)»,
//! продажа «монета → промежуточная (P2) → USDT (P1)» — в одной транзакции. Собирает два хопа в порядке исполнения и
//! отдаёт их общему расчёту маршрута (quote/route_calc.rs): каждый шаг по реальной ликвидности своего пула (та же
//! математика, что у прямого режима), со своей комиссией, крючком и налогом монеты на её стороне. Двухшаговый путь —
//! частный случай графа глубины 2 (решение 80): множитель-mid здесь один, расчёт пути — общий.
//! Кто вызывает: quote/via_pools.rs (лучшая пара), quote/smart.rs (доля через промежуточную), exec/live.rs; quote/bound_set.rs — константа CLASSIC_MAX_HOPS (ревью t162, Н8).
//! Пулы шагов, допуск промежуточной монеты и выбор лучшей пары — quote/via_pools.rs.
//! От чего зависит: quote/route_calc.rs, quote/pool_tax.rs (налог пула шага монеты, решение 146), quote/path.rs, quote/select.rs, pools/registry.rs, pools/mids.rs.
//! Решение 336: calc_from / first_leg — обмен пула монеты P2 один раз на пул, а не на каждую пару (via_pools.rs).
//! Настройки: через вызываемые — см. их шапки.
//!
//! Промежуточная монета должна быть проверена и без налогов (покупки, продажи, перевода): налог промежуточной на
//! переходе между пулами в одной транзакции официально не описан — не гадаем, такую монету не используем. Монета с
//! налогом — только пулы V2 на каждом шаге (решение 39; research/flow/bsc_smart_router.md, раздел 4.1).

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Side, TokenTaxes};
use crate::gateway::errors::GatewayResult;

use super::super::math::swap_cl::Swapped;
use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::Quoting;
use super::path::Path;
use super::pool_tax;
use super::route_calc::{self, Leg};
use super::route_first;
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

/// Ревью t162, Н8: наибольшее число хопов классики (direct / smart): прямой пул — 1, путь через промежуточную — ровно
/// столько хопов (legs собирает массив этой длины — классика длиннее не соберётся без смены константы). Её же берёт
/// строгий верх курса DEX как предел хопов классики (quote/bound_set.rs hop_limit) — граница и маршрут из одного места.
pub const CLASSIC_MAX_HOPS: usize = 2;

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
        self.calc_from(m, mid_dec, p1, p2, qty, None)
    }

    /// Решение 336: то же, что calc, с готовым обменом пула монеты P2 (first_leg; None — как calc). Ошибки: как calc.
    pub fn calc_from(
        &self,
        m: &Mid,
        mid_dec: u8,
        p1: &Ok2,
        p2: &Ok2,
        qty: Decimal,
        first: Option<&Swapped>,
    ) -> GatewayResult<Path> {
        let legs = self.legs(m, mid_dec, p1, p2);
        // Решение 146: налог монеты — пула её шага (P2), не монеты целиком (quote/pool_tax.rs).
        let tax_keep = pool_tax::coin_keep(&legs, self.side, self.taxes);
        route_calc::eval_from(&self.q.ctx, &legs, self.side, qty, tax_keep, first)
    }

    /// Решение 336: обмен пула монеты P2 пути через m (у покупки — последний хоп, у продажи — первый; route_calc.rs
    /// считает его первым) — зависит только от P2 и объёма. Принимает: как calc. Возвращает: обмен. Ошибки: как у
    /// первой итерации calc.
    pub fn first_leg(
        &self,
        m: &Mid,
        mid_dec: u8,
        p1: &Ok2,
        p2: &Ok2,
        qty: Decimal,
    ) -> GatewayResult<Swapped> {
        let legs = self.legs(m, mid_dec, p1, p2);
        let tax_keep = pool_tax::coin_keep(&legs, self.side, self.taxes);
        route_first::first(&self.q.ctx, &legs, self.side, qty, tax_keep)
    }

    /// Хопы пары пулов в порядке исполнения стороны; налог монеты — на её стороне (покупка — выход P2, продажа — вход
    /// P2). Массив длины CLASSIC_MAX_HOPS (ревью t162, Н8). Ошибок нет.
    fn legs(&self, m: &Mid, mid_dec: u8, p1: &Ok2, p2: &Ok2) -> [Leg; CLASSIC_MAX_HOPS] {
        let leg = |p: &Ok2, (token_in, token_out), (dec_in, dec_out)| Leg {
            entry: p.0.clone(),
            usable: p.1.clone(),
            token_in,
            token_out,
            dec_in,
            dec_out,
        };
        match self.side {
            Side::Buy => [
                leg(p1, (self.usdt, m.address), (self.usdt_dec, mid_dec)),
                leg(p2, (m.address, self.coin), (mid_dec, self.coin_dec)),
            ],
            Side::Sell => [
                leg(p2, (self.coin, m.address), (self.coin_dec, mid_dec)),
                leg(p1, (m.address, self.usdt), (mid_dec, self.usdt_dec)),
            ],
        }
    }
}
