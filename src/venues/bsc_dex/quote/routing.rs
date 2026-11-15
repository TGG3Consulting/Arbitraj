//! Файл: venues/bsc_dex/quote/routing.rs
//! Что делает: выбор маршрута котировки — классика (dex.swap_mode: direct / smart) и, при dex.router_mode = graph,
//! граф-роутер с выбором котировки-на-сделку среди исполнимых (graph_route::merge, решение 84 вариант Б). Это CPU-часть
//! котировки; она владеет своими данными, чтобы идти вне рабочих потоков tokio (quote/cpu.rs, решение 129 часть 2).
//! Логика та же, что была в quote/mod.rs, — перенесена без изменений.
//! Решение 271: классика ответила NoRoute (у монеты пулы только на форках / без классического пути), а
//! dex.router_mode = graph — считается граф; его ИСПОЛНИМЫЙ маршрут и есть котировка, иначе — прежняя ошибка классики.
//! Ревью 271: маршрут графа берётся, только если его внутренние токены — включённые промежуточные монеты без налогов
//! (Via::mid_ok, та же проверка, что у исполнения: exec/live.rs, exec/live_hops.rs; классика её делает в via_pools.rs).
//! Решение 294: при dex.intermediate_partial_fill = да и неполной классике (fillable < qty) в режиме graph — выбор
//! route_fit.rs merge (исполнимый граф с mids_ok наравне с классикой по pick, неисполнимый — signal_route, ошибка
//! графа NoRoute — классика без записи журнала, другая ошибка графа — прежняя запись); не задана / «нет» / объём помещается — graph_route::merge, как было.
//! Решение 300: dex.graph_exec_fallback = да и котировка решения/исполнения (Quoting.exec_quote) — лучший маршрут графа
//! неисполним → лучший исполнимый маршрут графа (quote/graph_fit.rs) против выбранной котировки (pick); в ветке 271 и
//! 294 — с той же проверкой mids_ok. Не задана / «нет» — Fit нет, всё как было.
//! Кто вызывает: quote/mod.rs (Quoting::quote через cpu::run).
//! От чего зависит: quote/{smart_quote, classic_direct, graph_route, via, steps}.rs, pools/registry.rs, pools/mids.rs.
//! Настройки: dex.router_mode, dex.intermediate_partial_fill (решение 294; только при неполной классике), dex.graph_exec_fallback (решение 300) [bsc_dex]; через вызываемые — dex.*, smart.* (см. их шапки).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Asset, DexRoute, Quote, Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::steps::Names;
use super::via::Via;
use super::{Quoting, classic_direct, graph_fit, graph_route, smart_quote};

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
/// чаевых (их добавляет quote/mod.rs). Ошибки: как у smart_quote / classic_direct (решение 271: NoRoute классики при
/// dex.router_mode = graph — только если и граф не дал исполнимый маршрут).
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
    let classic = if i.smart {
        smart_quote::quote(&v, &names, asset, qty, i.entries, &i.mids, gas).await
    } else {
        // Форки из классического ПРЯМОГО пути исключаем (ревью #86): форк исполним ТОЛЬКО через graph→bundle
        // (свой роутер). Решение 121: так же — неисполнимые семейства (tradable = нет): они не выигрывают
        // котировку как Direct, а видны только как signal_route (quote/classic_direct.rs).
        classic_direct::quote(&v, &names, asset, qty, i.entries, &i.mids, &|_| true, gas).await
    };
    let graph_on = ctx.text("dex.router_mode").ok().as_deref() == Some("graph");
    let mut out = match classic {
        Ok(c) => c,
        // Решение 271: классического пути нет (пулы монеты только на форках и т.п.), режим graph — котирует граф.
        // Берём только ИСПОЛНИМЫЙ граф-маршрут, чьи промежуточные прошли бы проверку исполнения (mids_ok);
        // иначе или ошибка графа — прежняя ошибка классики как есть.
        Err(e) if graph_on && e.code == ErrorCode::NoRoute => {
            let fit = graph_fit::enabled(&q);
            return match graph_route::quote_fit(&v, &names, asset, qty, gas, fit).await {
                Ok((g, _))
                    if g.route
                        .as_ref()
                        .is_some_and(|r| r.executable && mids_ok(&v, r, &i.mids)) =>
                {
                    Ok(g)
                }
                // Решение 300: лучший неисполним — лучший исполнимый маршрут графа (с той же проверкой mids_ok).
                Ok((_, Some(f)))
                    if f.quote
                        .route
                        .as_ref()
                        .is_some_and(|r| mids_ok(&v, r, &i.mids)) =>
                {
                    Ok(graph_fit::finish(&q, f, i.side, i.usdt_dec).await)
                }
                _ => Err(e),
            };
        }
        Err(e) => return Err(e),
    };
    // Граф-роутер (решение 80) за переключателем dex.router_mode: «не задано» / «classic» — поведение классики
    // неизменно; «graph» — считаем маршрут по графу и выбираем котировку-на-сделку среди ИСПОЛНИМЫХ маршрутов
    // (graph_route::merge, решение 84 вариант Б): неисполнимый граф-маршрут на сделку не идёт (остаётся лучшим
    // сигналом для карточки, PR 80.5b), поэтому graph-режим не исполняет хуже классики. Деньги не рискуют.
    if graph_on {
        // Решение 300: при dex.graph_exec_fallback = да на котировке решения/исполнения — и лучший исполнимый граф (fit).
        let fit_on = graph_fit::enabled(&q);
        let (g, fit) =
            graph_fit::parts(graph_route::quote_fit(&v, &names, asset, qty, gas, fit_on).await);
        // Решение 294: классика неполная (fillable < qty) и dex.intermediate_partial_fill = да — выбор среди
        // исполнимых; ошибка графа NoRoute — без записи Warn на каждую котировку (route_fit.rs merge). Иначе — merge, как было.
        if out.fillable_qty < qty && ctx.flag("dex.intermediate_partial_fill").unwrap_or(false) {
            let ok = g.as_ref().ok().and_then(|g| g.route.as_ref());
            let ok = ok.is_some_and(|r| r.executable && mids_ok(&v, r, &i.mids));
            let fit = fit.filter(|f| {
                f.quote
                    .route
                    .as_ref()
                    .is_some_and(|r| mids_ok(&v, r, &i.mids))
            });
            let out = super::route_fit::merge(&q, out, g, ok, i.side, i.usdt_dec).await;
            return Ok(graph_fit::merge(&q, out, fit, i.side, i.usdt_dec).await);
        }
        out = graph_route::merge(&q, out, g, i.side, i.usdt_dec).await;
        out = graph_fit::merge(&q, out, fit, i.side, i.usdt_dec).await;
    }
    Ok(out)
}

/// Ревью 271: внутренние токены каждой доли маршрута (выход не последнего хопа) — включённые промежуточные монеты без
/// налогов (Via::mid_ok), как требует исполнение (exec/live.rs — пара через промежуточную, exec/live_hops.rs — N хопов).
/// Цепочка токенов — от старта стороны (покупка — USDT, продажа — монета): выход хопа — другой конец его пула.
/// Принимает: расчёт путей, маршрут, промежуточные (включённые сейчас). Возвращает: true — проходит. Ошибок нет
/// (пул не в реестре или цепочка разорвана — false: исполнение такой маршрут тоже отвергнет).
fn mids_ok(v: &Via<'_>, route: &DexRoute, mids: &[(Mid, u8)]) -> bool {
    let start = match v.side {
        Side::Buy => v.usdt,
        Side::Sell => v.coin,
    };
    route.legs.iter().all(|leg| {
        let mut running = start;
        leg.hops.iter().enumerate().all(|(n, h)| {
            let Some(e) = v.q.reg.get(&h.pool) else {
                return false;
            };
            running = if e.usdt == running {
                e.coin
            } else if e.coin == running {
                e.usdt
            } else {
                return false;
            };
            n + 1 == leg.hops.len()
                || mids
                    .iter()
                    .find(|(m, _)| m.address == running)
                    .is_some_and(|(m, _)| v.mid_ok(m).is_ok())
        })
    })
}
