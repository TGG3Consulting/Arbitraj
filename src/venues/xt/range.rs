//! Файл: venues/xt/range.rs
//! Что делает: фильтр PRICE или QUANTITY пары XT.com — пределы, шаг tickSize (от минимума), точность пары; округление
//! значения до шага и проверка пределов.
//! Кто вызывает: symbols.rs (SymbolRules), symbols_parse.rs (разбор), order_rules.rs (округление).
//! От чего зависит: rust_decimal.
//! Настройки: нет.
//!
//! Официально (https://doc.xt.com/#marketsymbol): «price must be equal to minPrice+(integer multiple of tickSize)», то
//! же у количества; «Each of the above items can be null, when it is null, it means that this item is no longer
//! restricted». Нет tickSize — шаг по pricePrecision / quantityPrecision (ORDER_008 «precision is abnormal»).

use rust_decimal::{Decimal, RoundingStrategy};

/// Фильтр PRICE или QUANTITY: пределы (None — без ограничения), шаг tickSize, точность пары.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Range {
    pub min: Option<Decimal>,
    pub max: Option<Decimal>,
    pub tick: Option<Decimal>,
    /// pricePrecision / quantityPrecision — знаков после запятой.
    pub precision: Option<u32>,
}

impl Range {
    /// Шаг значения: tickSize, а если его нет — 10^−precision; None — ни того, ни другого. Ошибок нет.
    pub fn step(&self) -> Option<Decimal> {
        self.tick
            .or_else(|| self.precision.map(|p| Decimal::new(1, p.min(28))))
    }

    /// Что делает: округление до шага от минимума («min + целое число шагов», документация) в сторону how.
    /// Шага нет — без округления. Ошибок нет.
    pub fn round(&self, v: Decimal, how: RoundingStrategy) -> Decimal {
        let Some(step) = self.step() else {
            return v;
        };
        let base = match self.tick {
            Some(_) => self.min.unwrap_or(Decimal::ZERO),
            None => Decimal::ZERO,
        };
        let n = ((v - base) / step).round_dp_with_strategy(0, how);
        (base + n * step).normalize()
    }

    /// Значение вне пределов. Ошибок нет.
    pub fn outside(&self, v: Decimal) -> bool {
        self.min.is_some_and(|m| v < m) || self.max.is_some_and(|m| v > m)
    }
}
