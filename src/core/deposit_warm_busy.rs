//! Файл: core/deposit_warm_busy.rs
//! Что делает: решение 316 (доработка по ревью) — «прогрев уступает сделке»: какие биржи сейчас заняты сделкой, чтобы
//! фоновый прогрев адресов депозита не слал им запросов. Факт кода: у MEXC (venues/mexc/reply.rs PAUSE_UNTIL_MS) и
//! Gate (venues/gate/reply.rs) пауза после HTTP 429 — на ВЕСЬ REST биржи; 429 от прогрева остановил бы постановку
//! ордеров, вывод и опрос идущей сделки (таблица по биржам — DECISIONS 316). Поэтому прогрев проверяет занятость перед
//! КАЖДЫМ запросом (deposit_warm_pass.rs) — для всех бирж одинаково, ядро о площадках не знает.
//! Биржа занята, если по готовым признакам ядра (новых глобальных состояний нет):
//! - открытая сделка (Inner::deals, Deal::is_open) — её площадки покупки и продажи;
//! - идёт возврат USDT закрытой сделки (часть «идёт» — UsdtReturn::Pending) — площадки «откуда» и «куда»;
//! - монета занята (Inner::active, решение 118 Я-2: замок ставится ДО предпроверки — executor.rs accepted — и держится
//!   до конца сделки; так же ручные действия) — все площадки торгуемых направлений этой монеты по справочнику (замок
//!   знает монету, а не направление, поэтому с запасом);
//! - направление проходит расчёт в открытом окне (Windows::all_passing, решение 125) — его площадки.
//!
//! Не покрыто: ручной перевод USDT (Inner::usdt_moves — ключ отправителя не площадка ядра); сделка, начатая в те
//! миллисекунды, пока запрос прогрева уже в пути, — запрос не отзывается.
//! Кто вызывает: core/deposit_warm_pass.rs (snapshot — перед проходом и перед каждым запросом), deposit_warm_plan.rs
//! (pick — Busy::has), тест tests/core_deposit_warm_316.rs.
//! От чего зависит: engine.rs (lk, coins), deal.rs, usdt_return_parts.rs, window.rs, coin_key.rs, core/types.
//! Настройки: нет.

use std::collections::HashSet;

use crate::core::types::{Direction, VenueId};

use super::coin_key::CoinKey;
use super::engine::{Core, lk};
use super::usdt_return_state::UsdtReturn;

/// Биржи, занятые сделкой, — снимок на момент проверки.
#[derive(Debug, Default, Clone)]
pub struct Busy(HashSet<VenueId>);

impl Busy {
    /// Что делает: отмечает площадки направления (покупки и продажи). Ошибок нет.
    pub fn add_direction(&mut self, d: &Direction) {
        self.0.insert(d.buy_on.clone());
        self.0.insert(d.sell_on.clone());
    }

    /// Что делает: отмечает одну площадку. Ошибок нет.
    pub fn add(&mut self, v: &VenueId) {
        self.0.insert(v.clone());
    }

    /// Что делает: занята ли биржа — прогрев ей запросов не шлёт. Ошибок нет.
    pub fn has(&self, v: &VenueId) -> bool {
        self.0.contains(v)
    }
}

/// Что делает: снимок занятых бирж по признакам ядра (см. шапку). Блокировки короткие, по очереди, без await.
/// Принимает: ядро. Возвращает: Busy. Ошибок нет.
pub fn snapshot(core: &Core) -> Busy {
    let mut b = Busy::default();
    {
        let deals = lk(&core.inner.deals);
        for d in deals.values() {
            if d.is_open() {
                b.add_direction(&d.direction);
            }
            for p in super::usdt_return_parts::all(d) {
                if let UsdtReturn::Pending { from, to, .. } = p {
                    b.add(from);
                    b.add(to);
                }
            }
        }
    }
    let active: HashSet<CoinKey> = lk(&core.inner.active).clone();
    if !active.is_empty() {
        let book = core.coins();
        for c in book
            .all()
            .filter(|c| active.contains(&CoinKey::of(&c.asset)))
        {
            c.directions().iter().for_each(|d| b.add_direction(d));
        }
    }
    let passing = lk(&core.inner.windows).all_passing();
    passing.iter().for_each(|d| b.add_direction(d));
    b
}
