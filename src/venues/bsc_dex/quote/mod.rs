//! Файл: venues/bsc_dex/quote/mod.rs
//! Что делает: котировка на объём для контракта quote — режим dex.swap_mode: «direct» (лучший одиночный пул или пара
//! пулов через промежуточную монету, решения 29, 52 — quote/direct_route.rs) или «smart» (умный маршрут PancakeSwap против прямой сделки Uniswap, решение 38 —
//! quote/smart_quote.rs). Проверяет свежесть блока («не торгуем вслепую»), добавляет газ разрешений и чаевые.
//! Цена газа = max(eth_gasPrice с надбавкой, минимум газа готовых каналов) — та же, что при отправке.
//! Нативная монета BNB (решение 62): газ обёртки — quote/native_gas.rs; у продажи BNB разрешений нет (approve_gas.rs).
//! Кто вызывает: mod.rs (контракт quote).
//! От чего зависит: quote/direct.rs, quote/smart_quote.rs, quote/costs.rs, quote/gas.rs, check/state.rs,
//! pools/registry.rs, tokens.rs, node/heads.rs, wallet/approve.rs, send/mod.rs (минимум газа и чаевые каналов).
//! Настройки: dex.swap_mode, venue.quote_asset [bsc_dex]; разрешения — dex.universal_router, dex.permit2 [вид пула]
//! (прямой маршрут) или smart.universal_router, smart.permit2 [bsc_dex] (умный); feed.stale_after_ms [bsc_dex].

pub mod approve_gas;
pub mod costs;
pub mod cycle;
pub mod cycle_scan;
pub mod cycle_view;
pub mod depth_band;
pub mod direct;
pub mod direct_route;
pub mod emptiness;
pub mod gas;
pub mod graph_legs;
pub mod graph_route;
pub mod mixed_quoter;
pub mod native_gas;
pub mod path;
pub mod pick;
pub mod pmm_depth;
pub mod pool_quote;
pub mod quoter;
pub mod quoter_calls;
pub mod quoter_revert;
pub mod rate;
pub mod route_calc;
pub mod route_paths;
pub mod select;
pub mod smart;
pub mod smart_quote;
pub mod smart_ways;
pub mod spot;
pub mod steps;
pub mod verify;
pub mod verify_decide;
pub mod via;
pub mod via_pools;

use rust_decimal::Decimal;

use alloy_primitives::U256;

use crate::core::types::{Asset, Cost, CostKind, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::check::state::Checks;
use super::ctx::Ctx;
use super::node::heads::Heads;
use super::num;
use super::pools::mids::Mids;
use super::pools::registry::Registry;
use super::tokens::Tokens;
use super::wallet::balances::BNB;
use gas::GasPrice;
use steps::Names;
use via::Via;

/// Всё, что нужно котировке. Дешёво клонируется.
#[derive(Clone)]
pub struct Quoting {
    pub ctx: Ctx,
    pub reg: Registry,
    pub checks: Checks,
    pub tokens: Tokens,
    pub heads: Heads,
    pub gas: GasPrice,
    /// Промежуточные монеты (решение 52).
    pub mids: Mids,
    /// Разрешения кошелька в памяти до его транзакции (wallet/allow_cache.rs).
    pub allow: super::wallet::allow_cache::Allowances,
    /// «Данные пулов отстают» (решение 72): котировки нет, пока лента квитанций не догнала.
    pub lag: super::node::events_lag::EventsLag,
}

impl Quoting {
    /// Котировка. Принимает: монету, сторону, количество монет.
    /// Ошибки: NotConfigured (режим, тикер USDT), StaleData (нет свежего блока), CoinNotChecked / CoinCheckFailed,
    /// NoRoute (нет допустимого пула — причины по пулам), узел (decimals, цена газа).
    pub async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        let ctx = &self.ctx;
        let mode = ctx.text("dex.swap_mode")?;
        if mode != "direct" && mode != "smart" {
            return Err(ctx.not_set(
                "dex.swap_mode",
                format!("dex.swap_mode = «{mode}»: известны «direct» и «smart»"),
            ));
        }
        self.heads.fresh(ctx)?;
        self.lag.check(ctx)?;
        // Налоги — до любого запроса к узлу (decimals, цена газа, разрешения): не проверены — сразу отказ.
        let taxes = direct::taxes(self, asset)?;
        let usdt = self.tokens.usdt(ctx)?;
        let coin = asset.bsc_contract.ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                format!("{}: нет адреса контракта в BSC", asset.ticker),
            )
        })?;
        let native = super::exec::native::is_native(ctx, asset);
        // Пулы монеты с USDT; пулы с промежуточными монетами считает quote/via.rs (решение 52).
        let entries: Vec<_> = self
            .reg
            .by_pair(coin, usdt)
            .into_iter()
            .filter(|e| e.ticker == asset.ticker)
            // BNB (решение 62) — только пулы V2/V3: путь через V4 / Infinity не проверен живой записью.
            .filter(|e| !native || super::exec::native::pool_ok(e.pool.kind))
            .collect();
        let coin_dec = self.tokens.decimals(ctx, coin).await?;
        let usdt_dec = self.tokens.decimals(ctx, usdt).await?;
        let mut mids = Vec::new();
        for m in self.mids.active(ctx) {
            let d = self.tokens.decimals(ctx, m.address).await?;
            mids.push((m, d));
        }
        // Цена газа — как при отправке: eth_gasPrice с надбавкой, не ниже минимума готовых каналов (exec/txs.rs).
        let gas = self
            .gas
            .wei(ctx, &self.heads)
            .await?
            .max(super::send::planned_floor(ctx)?);
        let usdt_ticker = ctx.text("venue.quote_asset")?;
        let v = Via {
            q: self,
            coin,
            usdt,
            side,
            taxes: &taxes,
            coin_dec,
            usdt_dec,
        };
        let names = Names {
            usdt: (usdt, &usdt_ticker),
            coin: (coin, &asset.ticker),
            mids: &self.mids,
        };
        // Классика (решения 29, 38): прямой или умный режим. При dex.router_mode = graph ниже сравним с графом.
        let mut q = if mode == "smart" {
            smart_quote::quote(&v, &names, asset, qty, entries, &mids, gas).await?
        } else {
            // Форки из классического ПРЯМОГО пути исключаем (ревью #86): форк исполним ТОЛЬКО через graph→bundle
            // (свой роутер), а прямой путь строит план Universal Router (plan::build, нужны UR/Permit2, которых у
            // форка нет). Проба налога форка (probe.rs::part) теперь безусловна и делает форк-пул usable, поэтому без
            // этого фильтра форк мог бы выиграть классическую котировку как RouteMode::Direct и упасть на plan::build.
            // Умный путь уже отсекает форк (smart_ok); граф ведёт форк своим путём (не через этот keep). Не-форки — как прежде.
            direct_route::quote(
                &v,
                &names,
                asset,
                qty,
                entries,
                &mids,
                &|e| !e.family.fork,
                gas,
            )
            .await?
        };
        // Граф-роутер (решение 80) за переключателем dex.router_mode: «не задано» / «classic» — поведение классики
        // неизменно; «graph» — считаем маршрут по графу и выбираем котировку-на-сделку среди ИСПОЛНИМЫХ маршрутов
        // (graph_route::merge, решение 84 вариант Б): неисполнимый граф-маршрут на сделку не идёт (остаётся лучшим
        // сигналом для карточки, PR 80.5b), поэтому graph-режим не исполняет хуже классики. Деньги не рискуют.
        if ctx.text("dex.router_mode").ok().as_deref() == Some("graph") {
            let g = graph_route::quote(&v, &names, asset, qty, gas).await;
            q = graph_route::merge(self, q, g, side, usdt_dec).await;
        }
        // Газ разрешений и чаевые — к первому шагу маршрута (разрешения — на токен входа первого шага).
        let first = q.costs.iter().find_map(|c| c.step.clone());
        let with_step = |c: Cost| match &first {
            Some(s) => c.with_step(s.clone()),
            None => c,
        };
        if let Some(units) = approve_gas::units(self, &q, coin_dec, usdt_dec).await? {
            let wei = gas.saturating_mul(U256::from(units));
            let bnb = num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("Газ разрешений {wei} wei не переводится в BNB"),
                )
            })?;
            q.costs
                .push(with_step(Cost::new(CostKind::ApproveGas, bnb, BNB, false)));
        }
        // Нативная монета BNB (решение 62): газ WRAP_ETH / UNWRAP_WETH сверх обмена токеном — quote/native_gas.rs.
        if let Some((c, units)) = native_gas::cost(self, &q, gas)? {
            q.costs.push(with_step(c));
            if let Some(r) = q.route.as_mut() {
                r.gas_units += units;
            }
        }
        if let Some(tip) =
            super::send::planned_tip(ctx).and_then(|w| num::to_decimal(w, num::BNB_DECIMALS))
        {
            q.costs
                .push(with_step(Cost::new(CostKind::ChannelTip, tip, BNB, false)));
        }
        Ok(q)
    }
}
