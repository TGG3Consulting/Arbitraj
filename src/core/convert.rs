//! Файл: core/convert.rs
//! Что делает: перевод затрат в USDT по живым ценам (решение 35): USDT — как есть, монета сделки —
//! по её цене в этой сделке, BNB (газ, чаевые) — по живому стакану BNB-USDT биржи: котировка шлюза
//! на нужное количество BNB (адаптер проверяет свежесть, устарело — ошибка, сделки нет).
//! Кто вызывает: core/costs.rs, позже ledger.rs (факт сделки).
//! От чего зависит: core/types, deps.rs (шлюз), settings.
//! Настройки: venue.quote_asset (котируемая монета площадки).

use rust_decimal::Decimal;

use crate::core::types::{Asset, Cost, Side, VenueId};

use super::deps::Deps;

/// Монета газа сети BSC. Задана протоколом сети (газ платится в BNB) и стыком:
/// core/types/cost.rs — `Cost.asset` для газа и чаевых — «BNB».
pub const GAS_TICKER: &str = "BNB";

/// Монета газа как актив для подписки на стакан биржи (у нативной монеты нет адреса контракта).
pub fn gas_asset() -> Asset {
    Asset {
        ticker: GAS_TICKER.to_string(),
        bsc_contract: None,
    }
}

/// Курсы для перевода затрат одной сделки.
pub struct Rates {
    /// Котируемые монеты площадок сделки (venue.quote_asset) — считаются равными USDT.
    pub quote: Vec<String>,
    /// Тикер монеты сделки и её цена в USDT.
    pub coin: String,
    pub coin_price: Decimal,
    /// Цена BNB в USDT; None — газа в сделке нет, цена не нужна.
    pub bnb: Option<Decimal>,
}

/// Затрата в USDT. Принимает: затрату, курсы. Ошибки: затрата в BNB, а цены BNB нет;
/// затрата в неизвестной валюте (такую сделку не считаем — «не торгуем вслепую»).
pub fn to_usdt(c: &Cost, r: &Rates) -> Result<Decimal, String> {
    if r.quote.iter().any(|q| q == &c.asset) {
        Ok(c.amount)
    } else if c.asset == r.coin {
        Ok(c.amount * r.coin_price)
    } else if c.asset == GAS_TICKER {
        r.bnb
            .map(|p| c.amount * p)
            .ok_or_else(|| "нет цены BNB для пересчёта газа".to_string())
    } else {
        Err(format!(
            "затрата {:?} в {} — нет курса для пересчёта в USDT",
            c.kind, c.asset
        ))
    }
}

/// Сколько BNB среди затрат (газ, approve, перевод, чаевые). Ошибок нет.
pub fn bnb_total(costs: &[Cost]) -> Decimal {
    costs
        .iter()
        .filter(|c| c.asset == GAS_TICKER)
        .map(|c| c.amount)
        .sum()
}

/// Цена BNB в USDT по живому стакану биржи: средняя цена покупки нужного количества BNB.
/// Принимает: зависимости, биржи (по порядку — первая ответившая), количество BNB.
/// Возвращает: цену. Ошибки: ни одна биржа не дала свежую котировку BNB (причины перечислены).
pub async fn bnb_price(deps: &Deps, cex: &[VenueId], qty: Decimal) -> Result<Decimal, String> {
    let mut why = vec![];
    for v in cex {
        let q = match deps.gw.get(v).await {
            Ok(a) => a.quote(&gas_asset(), Side::Buy, qty).await,
            Err(e) => Err(e),
        };
        match q {
            Ok(q) if q.fillable_qty >= qty && !q.avg_price.is_zero() => return Ok(q.avg_price),
            Ok(q) => why.push(format!("{}: глубины BNB {} < {qty}", v.0, q.fillable_qty)),
            Err(e) => why.push(format!("{}: {}", v.0, e.message)),
        }
    }
    Err(format!("нет живой цены BNB: {}", why.join("; ")))
}
