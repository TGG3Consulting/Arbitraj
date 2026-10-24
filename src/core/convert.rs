//! Файл: core/convert.rs
//! Что делает: перевод затрат в USDT по живым ценам (решения 35, 52): котируемая монета — как есть,
//! монета сделки — по её цене в этой сделке, любая другая валюта (BNB — газ и чаевые; промежуточная
//! монета маршрута, например на шаге «USDT → BNB») — котировкой шлюза по живому стакану биржи на нужное
//! количество (для BNB — стакан BNB-USDT BingX). Живой цены нет — затрату не перевести, сделки нет.
//! Кто вызывает: core/costs.rs, trade_ops.rs, fills.rs, relay.rs, gas_stats.rs.
//! От чего зависит: core/types, deps.rs (шлюз).
//! Настройки: нет своих (котируемая монета — venue.quote_asset читают вызывающие).

use std::collections::HashMap;

use rust_decimal::Decimal;

use crate::core::types::{Asset, Cost, CostKind, Side, VenueId};

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

/// Газ сети и чаевые канала: платятся с BNB кошелька (резерв на газ, risk.min_bnb_balance), а не из монеты сделки —
/// даже когда монета сделки сама BNB (решение 62). Ошибок нет.
pub fn is_gas(k: CostKind) -> bool {
    matches!(
        k,
        CostKind::Gas
            | CostKind::ApproveGas
            | CostKind::TransferGas
            | CostKind::WrapGas
            | CostKind::ChannelTip
    )
}

/// Затрата в монете сделки — уменьшает количество (комиссия биржи в монете, налог): та же валюта и не газ.
/// Газ в BNB у сделки с BNB — не в количестве, а в затратах (решение 62). Ошибок нет.
pub fn in_coin(c: &Cost, coin: &str) -> bool {
    c.asset == coin && !is_gas(c.kind)
}

/// Затрата перегона USDT (решение 34) в расчёте: вид UsdtReturn — кроме газа: газ перевода остаётся газом (платится из
/// резерва BNB кошелька, в количество монет не идёт — in_coin; предпроверка резерва его видит — решение 62).
/// Принимает: затрату стоимости вывода USDT. Возвращает: затрату для строки «возврат USDT». Ошибок нет.
pub fn usdt_return(mut c: Cost) -> Cost {
    if !is_gas(c.kind) {
        c.kind = CostKind::UsdtReturn;
    }
    c
}

/// Курсы для перевода затрат одной сделки.
pub struct Rates {
    /// Котируемые монеты площадок сделки (venue.quote_asset) — считаются равными USDT.
    pub quote: Vec<String>,
    /// Тикер монеты сделки и её цена в USDT.
    pub coin: String,
    pub coin_price: Decimal,
    /// Живые цены остальных валют затрат (BNB, промежуточные монеты), USDT за единицу.
    pub prices: HashMap<String, Decimal>,
}

/// Затрата в USDT. Принимает: затрату, курсы. Ошибки: нет курса валюты затраты («не торгуем вслепую»).
pub fn to_usdt(c: &Cost, r: &Rates) -> Result<Decimal, String> {
    if r.quote.iter().any(|q| q == &c.asset) {
        Ok(c.amount)
    } else if c.asset == r.coin {
        Ok(c.amount * r.coin_price)
    } else if let Some(p) = r.prices.get(&c.asset) {
        Ok(c.amount * p)
    } else {
        Err(format!(
            "затрата {:?} в {} — нет живой цены для пересчёта в USDT",
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

/// Цена валюты в USDT по живому стакану биржи: средняя цена покупки нужного количества.
/// Принимает: зависимости, биржи (по порядку — первая ответившая), тикер, количество.
/// Возвращает: цену. Ошибки: ни одна биржа не дала свежую котировку (причины перечислены).
pub async fn price_of(
    deps: &Deps,
    cex: &[VenueId],
    ticker: &str,
    qty: Decimal,
) -> Result<Decimal, String> {
    let asset = Asset {
        ticker: ticker.to_string(),
        bsc_contract: None,
    };
    let mut why = vec![];
    for v in cex {
        let q = match deps.gw.get(v).await {
            Ok(a) => a.quote(&asset, Side::Buy, qty).await,
            Err(e) => Err(e),
        };
        match q {
            Ok(q) if q.fillable_qty >= qty && !q.avg_price.is_zero() => return Ok(q.avg_price),
            Ok(q) => why.push(format!(
                "{}: глубины {ticker} {} < {qty}",
                v.0, q.fillable_qty
            )),
            Err(e) => why.push(format!("{}: {}", v.0, e.message)),
        }
    }
    Err(format!("нет живой цены {ticker}: {}", why.join("; ")))
}

/// Цена BNB в USDT по живому стакану биржи (газ). Ошибки: как у price_of.
pub async fn bnb_price(deps: &Deps, cex: &[VenueId], qty: Decimal) -> Result<Decimal, String> {
    price_of(deps, cex, GAS_TICKER, qty).await
}

/// Собирает курсы для набора затрат: живая цена каждой валюты, кроме котируемых и монеты сделки,
/// на всё её количество в этих затратах. Ошибки: нет живой цены хотя бы одной валюты.
pub async fn rates(
    deps: &Deps,
    cex: &[VenueId],
    quote: Vec<String>,
    coin: &str,
    coin_price: Decimal,
    costs: &[Cost],
) -> Result<Rates, String> {
    let mut need: HashMap<String, Decimal> = HashMap::new();
    for c in costs
        .iter()
        .filter(|c| !quote.contains(&c.asset) && c.asset != coin)
    {
        *need.entry(c.asset.clone()).or_default() += c.amount;
    }
    let mut prices = HashMap::new();
    for (ticker, qty) in need.into_iter().filter(|(_, q)| !q.is_zero()) {
        prices.insert(ticker.clone(), price_of(deps, cex, &ticker, qty).await?);
    }
    Ok(Rates {
        quote,
        coin: coin.to_string(),
        coin_price,
        prices,
    })
}
