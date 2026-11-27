//! Файл: venues/bsc_dex/quote/mod.rs
//! Что делает: котировка на объём для контракта quote — режим dex.swap_mode: «direct» (лучший одиночный пул или пара
//! пулов через промежуточную монету, решения 29, 52 — quote/classic_direct.rs → direct_route.rs; неисполнимые пулы —
//! только сигнал, решение 121) или «smart» (умный маршрут PancakeSwap против прямой сделки Uniswap, решение 38 —
//! quote/smart_quote.rs). Проверяет свежесть блока («не торгуем вслепую»), добавляет газ разрешений и чаевые, а к
//! чаевым — газ их транзакции перевода (send/tip_gas.rs, Тигран 09.11: согласовано). Чаевые и их газ — только у пути
//! одной транзакцией; маршрут, который пойдёт бандлом свопов, их не несёт (exec/trade.rs sends_tip, Тигран 09.11).
//! Цена газа = max(eth_gasPrice с надбавкой, минимум газа готовых каналов) — та же, что при отправке.
//! Нативная монета BNB (решение 62): газ обёртки — quote/native_gas.rs; у продажи BNB разрешений нет (approve_gas.rs).
//! Выбор маршрута (классика, граф, merge) — quote/routing.rs, вне рабочих потоков tokio (quote/cpu.rs, решение 129).
//! Решение 273: пул Solidly маршрута на паузе фабрики или цена газа выше его maxGasPrice — маршрут неисполним
//! (executable = нет, причина; exec/solidly_gate.rs reason). Ревью 273: такой пул исключается уже при выборе пулов
//! (Quoting.plan_gas → quote/select.rs) — маршрут строится через другие пулы.
//! Решение 300: Quoting.exec_quote — котировка решения/исполнения (финальная или для исполнения) — для quote/graph_fit.rs.
//! Решение 312: замер котировки (quote/quote_timing_outer.rs Outer) — priced целиком: до работы (decimals, цена газа),
//! cpu::run (работа под quote_timing::timed — этапы в потоке счёта), после работы (газ разрешений, обёртки, чаевые);
//! запись dex.quote_timing — после хвоста (priced_at — прежнее тело priced, порядок шагов и ошибок тот же). Не задана /
//! нет — пустой Outer: часов нет, timed без замера.
//! Кто вызывает: mod.rs (контракт quote).
//! От чего зависит: quote/direct.rs, quote/smart_quote.rs, quote/costs.rs, quote/gas.rs, check/state.rs,
//! pools/registry.rs, tokens.rs, node/heads.rs, wallet/approve.rs, send/mod.rs (минимум газа и чаевые каналов),
//! exec/trade.rs (sends_tip — путь исполнения маршрута).
//! Настройки: dex.swap_mode, venue.quote_asset [bsc_dex]; разрешения — dex.universal_router, dex.permit2 [вид пула]
//! (прямой маршрут) или smart.universal_router, smart.permit2 [bsc_dex] (умный); feed.stale_after_ms [bsc_dex];
//! log.quote_timing, log.quote_timing_min_ms (вся система, решение 312).

pub mod abandon;
pub mod approve_bundle;
mod direct_allowance;
pub mod approve_gas;
pub mod basin_depth;
pub mod bound;
pub mod bound_cycle;
pub mod bound_graph;
pub mod bound_rate;
pub mod bound_set;
pub mod breaks;
pub mod breaks_bins;
pub mod breaks_hook;
pub mod breaks_pool;
pub mod camelot_quote;
pub mod classic_direct;
pub mod costs;
pub mod cpu;
pub mod cpu_local;
pub mod crypto_depth;
pub mod cycle;
pub mod cycle_audit;
mod cycle_cash;
mod cycle_reuse;
pub mod cycle_cost;
pub mod cycle_legs;
pub mod cycle_opt;
pub mod cycle_rank;
pub mod cycle_rate;
pub mod cycle_scan;
pub mod cycle_tally;
pub mod cycle_view;
mod delta_pricing;
mod sectorone;
mod umbrae;
pub mod depth_band;
pub mod depth_band_algebra;
pub mod depth_ekubo;
pub mod direct;
pub mod direct_route;
pub mod ekubo_trade_state;
pub mod emptiness;
pub mod fills;
pub mod fluid_depth;
pub(crate) mod frax_pricing;
pub(super) mod ekubo_v2_pricing;
pub(super) mod carbon_pricing;
mod temple_budget;
pub(super) mod mint_club_pricing;
mod module_pricing;
mod uniswap_v1_pricing;
pub(in crate::venues::bsc_dex) mod surge_pricing;
mod prop_pricing;
mod integral_pricing;
mod dagrow_pricing;
mod bundles_pricing;
mod bundles_route;
mod antfarm_pricing;
pub mod gas;
pub mod gas_base;
pub mod graph_exec;
pub mod graph_executor;
pub mod graph_fit;
pub mod graph_legs;
pub mod graph_reuse;
pub mod graph_route;
pub mod graph_seq;
pub mod graph_split;
pub mod hook_curve;
pub mod hook_rule;
pub mod hook_settle;
pub mod mixed_quoter;
pub mod mixed_quoter_calls;
mod nami_routes;
pub mod native_gas;
pub(super) mod oasis_pricing;
pub mod par;
pub mod par_pool;
pub mod path;
pub mod pick;
pub mod pmm_depth;
pub mod pmm_ladder;
pub mod pool_after;
pub mod pool_quote;
pub mod pool_tax;
pub mod probe_quote;
pub mod quote_timing;
pub mod quote_timing_log;
pub mod quote_timing_outer;
pub mod quote_timing_stages;
pub mod quoter;
pub mod quoter_balancer;
pub mod quoter_balancer_v3;
pub mod quoter_balancer_v3_buffer;
pub mod quoter_basin;
pub mod quoter_calls;
pub mod quoter_crypto;
pub mod quoter_dmm;
pub mod quoter_ekubo;
pub mod quoter_extra;
pub mod quoter_fluid;
pub mod quoter_lb;
pub mod quoter_lb20;
pub mod quoter_maverick;
pub mod quoter_revert;
pub mod quoter_shell;
pub mod quoter_wombat;
pub mod rate;
pub mod route_best;
pub mod route_cache;
pub mod route_calc;
pub mod route_first;
pub mod route_fit;
pub mod route_gas;
pub mod route_hops;
pub mod route_paths;
pub(super) mod router_pricing;
pub mod routing;
pub mod select;
pub mod shallow_build;
pub mod shallow_dirty;
pub mod shallow_memo;
pub mod shallow_pick;
pub mod shallow_stamps;
pub mod shell_depth;
pub mod side_fit;
pub mod smart;
pub mod smart_quote;
pub mod smart_ways;
pub mod solidly_depth;
pub mod spot;
pub mod stable_depth;
pub mod steps;
pub mod tick_bin_spot;
pub mod usable_reuse;
pub(super) mod v2_execution;
mod v2_fallback;
mod registered_v3_pricing;
mod market_selection;
pub(super) fn uses_wallet_quoter(variant: super::pools::family_variant::Variant) -> bool {
    market_selection::wallet_quoter(variant).is_some()
}
mod market_fees;
pub(super) mod vault_pricing;
pub mod v2_pair_calls;
mod candy_probe;
pub mod v2_pair_probe;
pub mod v2_pair_quote;
pub mod v2_pair_sim;
mod candy_wallet;
pub(super) mod v2_wallet_sim;
pub mod vault_depth;
pub mod verify;
pub mod verify_decide;
pub mod via;
pub mod via_pools;
pub mod virtual_quoter;
mod vista_cash;
pub(super) mod vista_pricing;
pub mod warm;
mod white_pricing;
pub(super) mod white_state;
pub mod wombat_depth;

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
use gas::GasPrice;
use quote_timing_outer::{Outcome, Outer};
use quote_timing_stages::Part;

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
    /// Решение 217: блок головы на момент снимка (замороженный реестр, frozen.rs); живая — None. Решение 301 (е): в ключ
    /// кэша графа больше не входит (блок в ключе графа убран) — сейчас только отметка снимка.
    pub frozen_head: Option<u64>,
    /// Ревью 273: плановая цена газа выбора маршрута, wei — пул Solidly с пределом цены газа фабрики ниже неё не
    /// допускается (quote/select.rs), как пул на паузе. None — не проверяется (вне выбора маршрута и поиска циклов).
    pub plan_gas: Option<U256>,
    /// Решение 300: котировка решения/исполнения — финальная котировка ядра (gateway::quote_stage::is_final) или
    /// котировка для исполнения (gateway::patience::is_exec); ставит priced. Только при ней — лучший исполнимый маршрут
    /// графа (quote/graph_fit.rs). false — подбор объёма, экран, пробы.
    pub exec_quote: bool,
    /// Решение «335 (В1, пакет)»: свежие записи пулов маршрута, перечитанные предторговой верификацией (решение 83) —
    /// ставит exec/trade.rs после неё; проба крючков перед сделкой (exec/trade_probe.rs hooks_again) берёт их на том же
    /// блоке вместо второго чтения. None — верификации не было (котировка, прямой и умный маршрут без графа).
    pub route_fresh: Option<std::sync::Arc<super::check::hook_batch::RouteFresh>>,
    /// Решение 382: пулы маршрута, перечитанные на голове после расхождения расчёта по памяти с квотером перед сделкой
    /// (exec/quoter_refetch.rs). Some — путь доли считается по этим записям (exec/live.rs, exec/live_hops.rs), квотер —
    /// на их блоке; None — по памяти, квотер на «latest», как раньше.
    pub pinned: Option<std::sync::Arc<super::check::hook_batch::RouteFresh>>,
}

impl Quoting {
    /// Котировка. Принимает: монету, сторону, количество монет.
    /// Ошибки: NotConfigured (режим, тикер USDT), StaleData (нет свежего блока), CoinNotChecked / CoinCheckFailed,
    /// NoRoute (нет допустимого пула — причины по пулам), узел (decimals, цена газа).
    pub async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        // Порядок проверок как до решения 217: режим, затем свежесть блока и ленты событий.
        self.swap_mode()?;
        self.heads.fresh(&self.ctx)?;
        self.lag.check(&self.ctx)?;
        self.priced(asset, side, qty).await
    }

    /// Режим котировки dex.swap_mode. Возвращает: «direct» или «smart».
    /// Ошибки: NotConfigured — не задан или неизвестное значение.
    fn swap_mode(&self) -> GatewayResult<String> {
        let mode = self.ctx.text("dex.swap_mode")?;
        if mode != "direct" && mode != "smart" {
            return Err(self.ctx.not_set(
                "dex.swap_mode",
                format!("dex.swap_mode = «{mode}»: известны «direct» и «smart»"),
            ));
        }
        Ok(mode)
    }

    /// Котировка без проверки свежести блока и ленты событий: у живой её делает quote, у замороженной (решение 217,
    /// frozen.rs) — снимок в момент решения. Принимает и ошибки: как quote (кроме свежести).
    pub async fn priced(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        // Решение 312: замер всей priced — одно чтение log.quote_timing; выключен — пустой, часов нет. Брошенный
        // вызывающим priced пишет запись сам (Drop Outer).
        let mut tm = Outer::begin(&self.ctx, &asset.ticker, side, qty);
        let out = self.priced_at(asset, side, qty, &mut tm).await;
        // Итог — замыкание: без замера Outcome не строится (решение 312).
        tm.finish(|| Outcome::of(&out));
        out
    }

    /// Тело priced под замером `tm` (решение 312: только часы вокруг шагов). Принимает и ошибки: как priced.
    async fn priced_at(
        &self,
        asset: &Asset,
        side: Side,
        qty: Decimal,
        tm: &mut Outer,
    ) -> GatewayResult<Quote> {
        let ctx = &self.ctx;
        let mode = self.swap_mode()?;
        // Налоги — до любого запроса к узлу (decimals, цена газа, разрешения): не проверены — сразу отказ.
        let taxes = direct::taxes(self, asset)?;
        let usdt = self.tokens.usdt(ctx)?;
        let coin = asset.contract.ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                format!("{}: нет адреса контракта на {}", asset.ticker, ctx.venue().0),
            )
        })?;
        let native = super::exec::native::is_native(ctx, asset);
        // Пулы монеты с USDT; пулы с промежуточными монетами считает quote/via.rs (решение 52).
        let entries: Vec<_> = self
            .reg
            .by_pair(coin, usdt)
            .into_iter()
            // Решение 211: пулы найдены по адресу монеты — фильтр по тикеру не нужен (показное имя монеты ядра может
            // не совпадать с тикером в реестре пулов).
            // BNB (решение 62) — только пулы V2/V3: путь через V4 / Infinity не проверен живой записью.
            .filter(|e| !native || super::exec::native::pool_ok(e.pool.kind))
            .collect();
        let t = tm.now();
        let coin_dec = self.tokens.decimals(ctx, coin).await?;
        let usdt_dec = self.tokens.decimals(ctx, usdt).await?;
        let mut mids = Vec::new();
        for m in self.mids.active(ctx) {
            let d = self.tokens.decimals(ctx, m.address).await?;
            mids.push((m, d));
        }
        tm.add(Part::Decimals, t);
        // Цена газа — как при отправке: eth_gasPrice с надбавкой, не ниже минимума готовых каналов (exec/txs.rs).
        let t = tm.now();
        let fees = self.gas.for_quote(ctx, &self.heads).await?;
        let gas = fees
            .wei(ctx, &self.heads)
            .await?
            .max(super::send::planned_floor(ctx)?);
        tm.add(Part::Gas, t);
        let usdt_ticker = ctx.text("venue.quote_asset")?;
        // Выбор маршрута — счёт CPU: вне рабочих потоков tokio (quote/cpu.rs, решение 129 часть 2), срок расчёта
        // ядра срабатывает вовремя. Логика выбора — quote/routing.rs (перенесена без изменений).
        let input = routing::Input {
            asset: asset.clone(),
            side,
            qty,
            coin,
            usdt,
            taxes: taxes.clone(),
            coin_dec,
            usdt_dec,
            entries,
            mids,
            gas,
            usdt_ticker,
            smart: mode == "smart",
        };
        // Ревью 273: пулы Solidly с пределом цены газа фабрики ниже этой цены газа исключаются при выборе пулов
        // (quote/select.rs по plan_gas) — маршрут считается без них.
        let mut pick = self.clone();
        pick.gas = fees.clone();
        pick.plan_gas = Some(gas);
        // Решение 300: признаки задачи tokio — здесь (счёт маршрута уходит в другой поток, quote/cpu.rs их не переносит).
        pick.exec_quote =
            crate::gateway::quote_stage::is_final() || crate::gateway::patience::is_exec();
        // Решение 301 (дополнение — стек): работа — в кучу здесь, состояние этого future держит указатель
        // (quote/cpu.rs run).
        // Решение 312: замер этапов работы — tm.run() = None (выключен): timed сразу ведёт работу без замера.
        self.market_routes(pick,input,&fees,native,tm).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish(
        &self,
        mut q: Quote,
        (coin_dec, usdt_dec): (u8, u8),
        gas: U256,
        fees: &GasPrice,
        native: bool,
        measured_approval: Option<(u64, usize)>,
        tm: &mut Outer,
    ) -> GatewayResult<Quote> {
        let ctx = &self.ctx;
        // Решение 273: пул Solidly маршрута на паузе фабрики или цена газа выше его maxGasPrice — маршрут неисполним
        // (сигнал с причиной; exec/trade.rs не исполняет executable = нет). После отбора по plan_gas — страховка.
        if let Some(r) = q.route.as_mut() {
            let used: Vec<_> = r
                .legs
                .iter()
                .flat_map(|l| &l.hops)
                .filter_map(|h| self.reg.get(&h.pool))
                .collect();
            if let Some(w) = super::exec::solidly_gate::reason(&used, gas) {
                r.executable = false;
                r.exec_note = Some(w);
            }
        }
        // Решение 416: тикер газа — dex.gas_ticker площадки (gas_ticker.rs).
        let gas_t = super::gas_ticker::read(ctx)?;
        // Газ разрешений и чаевые — к первому шагу маршрута (разрешения — на токен входа первого шага).
        let first = q.costs.iter().find_map(|c| c.step.clone());
        let with_step = |c: Cost| match &first {
            Some(s) => c.with_step(s.clone()),
            None => c,
        };
        let t = tm.now();
        let approvals = match measured_approval {
            Some((0, 0)) => None,
            Some(v) => Some(v),
            None => approve_gas::details(self, &q, coin_dec, usdt_dec).await?,
        };
        if let Some((units, count)) = approvals {
            let wei = gas
                .saturating_mul(U256::from(units))
                .saturating_add(fees.extra(ctx, units, count)?);
            let bnb = num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("Газ разрешений {wei} wei не переводится в BNB"),
                )
            })?;
            q.costs.push(with_step(Cost::new(
                CostKind::ApproveGas,
                bnb,
                &gas_t,
                false,
            )));
        }
        tm.add(Part::Approve, t);
        let t = tm.now();
        // Нативная монета BNB (решение 62): газ WRAP_ETH / UNWRAP_WETH сверх обмена токеном — quote/native_gas.rs.
        if let Some((c, units)) = native_gas::cost(self, &q, gas)? {
            q.costs.push(with_step(c));
            if let Some(r) = q.route.as_mut() {
                r.gas_units += units;
            }
        }
        if let Some(r) = &q.route {
            let extra = fees.extra(ctx, r.gas_units, 1)?;
            if !extra.is_zero() {
                let native_fee = num::to_decimal(extra, num::BNB_DECIMALS).ok_or_else(|| {
                    ctx.err(
                        ErrorCode::BadResponse,
                        "комиссия публикации и оператора Base слишком велика",
                    )
                })?;
                q.costs.push(with_step(Cost::new(
                    CostKind::Gas,
                    native_fee,
                    &gas_t,
                    false,
                )));
            }
        }
        tm.add(Part::Native, t);
        let t = tm.now();
        // Тигран 09.11 (согласовано, план = факт): чаевые и газ их перевода — только если путь исполнения маршрута их
        // отправляет (одна транзакция); бандл свопов графа их не шлёт (exec/trade.rs sends_tip — тот же выбор пути).
        let tip_sent = super::exec::trade::sends_tip(ctx, &self.reg, q.route.as_ref(), native);
        if let Some(tip) = super::send::planned_tip(ctx)
            .filter(|_| tip_sent)
            .and_then(|w| num::to_decimal(w, num::BNB_DECIMALS))
        {
            q.costs.push(with_step(Cost::new(
                CostKind::ChannelTip,
                tip,
                &gas_t,
                false,
            )));
            // Тигран 09.11 (согласовано): газ отдельной транзакции перевода чаевых (exec/txs.rs, nonce + 1, та же цена
            // газа) — та же статья, что в факте (core/deal_gas.rs «газ перевода чаевых», вид ChannelTip).
            if let Some(g) = super::send::tip_gas::planned_bnb(ctx, gas).await? {
                q.costs
                    .push(with_step(Cost::new(CostKind::ChannelTip, g, &gas_t, false)));
            }
        }
        tm.add(Part::Tip, t);
        Ok(q)
    }
}
