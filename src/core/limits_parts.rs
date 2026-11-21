//! Файл: core/limits_parts.rs
//! Что делает: составляющие бюджета сделки (limits.rs) — лимит на сделку, остаток лимита на монету, остаток лимита в
//! пути, виртуальный бюджет сухого режима — и какая из них ограничила (решение 141: расчёт «без потолка» пишет, что
//! срезало объём). Бюджет = меньшее из составляющих — та же формула, что и раньше в limits::budget (вынесена сюда).
//! Решение 267: в теневом расчёте занятой монеты (busy_calc.rs; признак задачи busy_budget.rs) остатки лимита на
//! монету и «в пути» считаются без вычета её текущей сделки; вне теневого расчёта — как раньше.
//! Кто вызывает: core/limits.rs (budget), core/uncapped.rs, core/busy_calc.rs.
//! От чего зависит: engine.rs (учёт Exposure), dry.rs (режим), busy_budget.rs, settings.
//! Настройки: money.max_per_trade_usd [площадка A; решение 242 — площадка + тип направления, venue_dir.rs], money.max_per_coin_usd, money.max_in_transit_usd, dry.budget_usdt
//! (через dry::Mode).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::Direction;
use crate::settings::read::At;

use super::engine::{Core, lk};

/// Составляющие бюджета, USDT.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Parts {
    /// money.max_per_trade_usd (площадка + тип направления, иначе площадка A — решение 242).
    pub per_trade: Decimal,
    /// money.max_per_coin_usd − вложено в открытые сделки монеты.
    pub coin_left: Decimal,
    /// money.max_in_transit_usd − в пути.
    pub transit_left: Decimal,
    /// Сухой режим: dry.budget_usdt; None — обычный режим.
    pub dry: Option<Decimal>,
}

impl Parts {
    /// Бюджет — меньшее из составляющих. Ошибок нет.
    pub fn cap(&self) -> Decimal {
        let c = self.per_trade.min(self.coin_left).min(self.transit_left);
        self.dry.map_or(c, |b| c.min(b))
    }

    /// Какая составляющая ограничила: per_trade / per_coin / in_transit / dry (при равенстве — первая по порядку).
    /// Ошибок нет.
    pub fn binding(&self) -> &'static str {
        let c = self.cap();
        if self.per_trade == c {
            "per_trade"
        } else if self.coin_left == c {
            "per_coin"
        } else if self.transit_left == c {
            "in_transit"
        } else {
            "dry"
        }
    }
}

/// Составляющие бюджета направления (баланс площадки не читается — решение 94). Принимает: ядро, направление, режим
/// (зафиксирован вызывающим). Ошибки: не задан лимит; в сухом режиме — не задан dry.budget_usdt.
pub fn parts(core: &Core, d: &Direction, mode: &super::dry::Mode) -> Result<Parts, String> {
    let s = &core.inner.deps.settings;
    // Решение 242: значение пары «площадка + тип направления» (vd), иначе прежняя цепочка (площадка, вся система).
    let g = |name: &str, venue: Option<&str>, vd| {
        s.decimal_dir(name, vd, At { venue, coin: None })
            .map_err(|e| e.message)
    };
    let vd = super::venue_dir::of(core, d);
    let per_trade = g("money.max_per_trade_usd", Some(&d.buy_on.0), vd)?;
    let per_coin = g("money.max_per_coin_usd", None, None)?;
    let transit = g("money.max_in_transit_usd", None, None)?;
    // Сухой режим: виртуальный бюджет вместо денег площадки (решение 74).
    let dry = match mode {
        super::dry::Mode::Dry(b) => Some(b.clone()?),
        super::dry::Mode::Live => None,
    };
    let ex = lk(&core.inner.exposure).clone();
    let mut p = Parts {
        per_trade,
        coin_left: per_coin - ex.per_coin.get(&d.asset).copied().unwrap_or_default(),
        transit_left: transit - ex.in_transit,
        dry,
    };
    // Решение 267: только в теневом расчёте занятой монеты (признак задачи busy_budget.rs) — без вычета её текущей
    // сделки; у кандидата, предпроверки и всех остальных признака нет — ничего не меняется.
    super::busy_budget::add_back(core, d, &mut p);
    Ok(p)
}
