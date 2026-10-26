//! Файл: core/card_book.rs
//! Что делает: цена и ликвидность стакана биржи для карточки сигнала (решение 74): на нужной стороне (покупка на бирже —
//! asks, продажа — bids) сумма в USDT уровней в пределах той же полосы цены от лучшей, что и глубина пулов
//! (dex.depth_band_pct площадки-DEX направления, решение 73). Почему полоса, а не «глубина на объём сделки»: так стакан и
//! пул сравниваются одной мерой (USDT до сдвига цены на b %), а глубина на объём уже видна в «максимальном объёме»
//! карточки. Если последний загруженный уровень ещё внутри полосы — сумма неполная (загружено venue.depth_levels
//! уровней), это видно (beyond).
//! Кто вызывает: core/card.rs, тесты (tests/core_signal_max.rs — записанный стакан BingX).
//! От чего зависит: core/types (OrderBook, PriceLevel, Side).
//! Настройки: dex.depth_band_pct (читает card.rs).

use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{PriceLevel, Side};

/// Стакан биржи в карточке.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BookSide {
    pub venue: String,
    /// Сторона сделки на этой площадке: Buy — берём asks, Sell — bids.
    pub side: Side,
    /// Лучшая цена нужной стороны.
    pub best: Option<Decimal>,
    /// Полоса, % (dex.depth_band_pct).
    pub band_pct: Option<Decimal>,
    /// Сумма в USDT уровней в полосе.
    pub usd_in_band: Option<Decimal>,
    /// Монет в полосе.
    pub qty_in_band: Option<Decimal>,
    /// Сколько уровней загружено на этой стороне.
    pub levels: usize,
    /// Полоса шире загруженных уровней: последний уровень ещё в полосе — сумма неполная.
    pub beyond: bool,
    /// Почему не посчитано (нет стакана, не задана полоса).
    pub error: Option<String>,
}

/// Что делает: сумма уровней в полосе от лучшей цены.
/// Принимает: уровни нужной стороны (от лучшей), сторону сделки, полосу в %.
/// Возвращает: (USDT, монет, вышла ли полоса за загруженные уровни); None — уровней нет. Ошибок нет.
pub fn in_band(
    levels: &[PriceLevel],
    side: Side,
    band_pct: Decimal,
) -> Option<(Decimal, Decimal, bool)> {
    let best = levels.first()?.price;
    let k = band_pct / Decimal::ONE_HUNDRED;
    let edge = match side {
        Side::Buy => best * (Decimal::ONE + k),
        Side::Sell => best * (Decimal::ONE - k),
    };
    let inside = |p: Decimal| match side {
        Side::Buy => p <= edge,
        Side::Sell => p >= edge,
    };
    let (mut usd, mut qty, mut n) = (Decimal::ZERO, Decimal::ZERO, 0usize);
    for l in levels.iter().take_while(|l| inside(l.price)) {
        usd += l.price * l.qty;
        qty += l.qty;
        n += 1;
    }
    Some((usd, qty, n == levels.len()))
}

/// Что делает: строка карточки по стакану. Принимает: площадку, сторону сделки, уровни нужной стороны (None — стакана
/// нет), полосу (Err — почему её нет). Возвращает: BookSide. Ошибок нет — в error.
pub fn side(
    venue: &str,
    side: Side,
    levels: Option<&[PriceLevel]>,
    band: Result<Decimal, String>,
) -> BookSide {
    let mut b = BookSide {
        venue: venue.to_string(),
        side,
        best: levels.and_then(|l| l.first()).map(|l| l.price),
        band_pct: band.as_ref().ok().copied(),
        usd_in_band: None,
        qty_in_band: None,
        levels: levels.map_or(0, <[PriceLevel]>::len),
        beyond: false,
        error: None,
    };
    match (levels, band) {
        (None, _) => b.error = Some("стакана нет".into()),
        (_, Err(e)) => b.error = Some(e),
        (Some(l), Ok(band)) => match in_band(l, side, band) {
            Some((usd, qty, beyond)) => {
                b.usd_in_band = Some(usd.round_dp(2));
                b.qty_in_band = Some(qty);
                b.beyond = beyond;
            }
            None => b.error = Some("стакан пуст".into()),
        },
    }
    b
}
