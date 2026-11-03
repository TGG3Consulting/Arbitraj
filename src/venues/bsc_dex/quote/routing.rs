//! Файл: venues/bsc_dex/quote/routing.rs
//! Что делает: выбор маршрута котировки — классика (dex.swap_mode: direct / smart) и, при dex.router_mode = graph,
//! граф-роутер с выбором котировки-на-сделку среди исполнимых (graph_route::merge, решение 84 вариант Б). Это CPU-часть
//! котировки; она владеет своими данными, чтобы идти вне рабочих потоков tokio (quote/cpu.rs, решение 129 часть 2).
//! Логика та же, что была в quote/mod.rs, — перенесена без изменений.
//! Кто вызывает: quote/mod.rs (Quoting::quote через cpu::run).
//! От чего зависит: quote/{smart_quote, classic_direct, graph_route, via, steps}.rs, pools/registry.rs, pools/mids.rs.
//! Настройки: dex.router_mode [bsc_dex]; через вызываемые — dex.*, smart.* (см. их шапки).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Asset, Quote, Side, TokenTaxes};
use crate::gateway::errors::GatewayResult;

use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::steps::Names;
use super::via::Via;
use super::{Quoting, classic_direct, graph_route, smart_quote};

/// Всё, что нужно выбору маршрута (владеет данными — работа уходит в другой поток).
pub struct Input {
    pub asset: Asset,
    pub side: Side,
    pub qty: Decimal,
    pub coin: Address,
    pub usdt: Address,
    pub taxes: TokenTaxes,
    pub coin_dec: u8,
    pub usdt_dec: u8,
    /// Пулы монеты с USDT (уже отфильтрованы по тикеру и BNB, решение 62).
    pub entries: Vec<Entry>,
    /// Промежуточные монеты с decimals (решение 52).
    pub mids: Vec<(Mid, u8)>,
    /// Цена газа, wei.
    pub gas: U256,
    /// Тикер USDT (venue.quote_asset).
    pub usdt_ticker: String,
    /// dex.swap_mode = smart.
    pub smart: bool,
}

/// Котировка маршрута. Принимает: котировку (клон), входные данные. Возвращает: котировку без газа разрешений и
/// чаевых (их добавляет quote/mod.rs). Ошибки: как у smart_quote / classic_direct.
pub async fn run(q: Quoting, i: Input) -> GatewayResult<Quote> {
    let ctx = &q.ctx;
    let v = Via {
        q: &q,
        coin: i.coin,
        usdt: i.usdt,
        side: i.side,
        taxes: &i.taxes,
        coin_dec: i.coin_dec,
        usdt_dec: i.usdt_dec,
    };
    let names = Names {
        usdt: (i.usdt, &i.usdt_ticker),
        coin: (i.coin, &i.asset.ticker),
        mids: &q.mids,
    };
    let (asset, qty, gas) = (&i.asset, i.qty, i.gas);
    // Классика (решения 29, 38): прямой или умный режим. При dex.router_mode = graph ниже сравним с графом.
    let mut out = if i.smart {
        smart_quote::quote(&v, &names, asset, qty, i.entries, &i.mids, gas).await?
    } else {
        // Форки из классического ПРЯМОГО пути исключаем (ревью #86): форк исполним ТОЛЬКО через graph→bundle
        // (свой роутер). Решение 121: так же — неисполнимые семейства (tradable = нет): они не выигрывают
        // котировку как Direct, а видны только как signal_route (quote/classic_direct.rs).
        classic_direct::quote(&v, &names, asset, qty, i.entries, &i.mids, &|_| true, gas).await?
    };
    // Граф-роутер (решение 80) за переключателем dex.router_mode: «не задано» / «classic» — поведение классики
    // неизменно; «graph» — считаем маршрут по графу и выбираем котировку-на-сделку среди ИСПОЛНИМЫХ маршрутов
    // (graph_route::merge, решение 84 вариант Б): неисполнимый граф-маршрут на сделку не идёт (остаётся лучшим
    // сигналом для карточки, PR 80.5b), поэтому graph-режим не исполняет хуже классики. Деньги не рискуют.
    if ctx.text("dex.router_mode").ok().as_deref() == Some("graph") {
        let g = graph_route::quote(&v, &names, asset, qty, gas).await;
        out = graph_route::merge(&q, out, g, i.side, i.usdt_dec).await;
    }
    Ok(out)
}
