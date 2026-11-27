//! Файл: venues/bsc_dex/venue.rs
//! Что делает: адаптер DEX в сети BSC — реализация контракта gateway/contract.rs (функции контракта; общее состояние —
//! parts.rs). Срочность запросов к узлу (решение 61, gateway/urgency.rs): сделка, вывод, сверки перевода и стоимость
//! вывода — Trade; котировка — класс ядра или Quote; остальное — фон.
//! Кто вызывает: main.rs (через mod.rs: создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings.
//! Настройки: docs/SETTINGS.md — «Узел BSC», «DEX», «Отправка транзакций», venue.* и feed.* с областью bsc_dex;
//! exec.via_kyber, exec.dex_best_of (решение 123, «лучший из двух» — best_of/).
//! Решение 132: деньги и адрес кошелька — только у вида на счёт (venue_account.rs); без привязки — отказ (Ctx::bound).
//! t78: продажа выхода сделки целиком или вывод её монеты — налоги кошелька сделки сняты (check/deal_taxes.rs).

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    Asset, Balance, CoinNetworkInfo, Cost, DexAccountId, DexRoute, PoolDirectory, Quote,
    SendChannelInfo, Side, TradeRequest, TradeResult, Transfer, VenueId, WalletQueue,
    WithdrawRequest,
};
use crate::gateway::contract::{ConnectionCheck, DexFamily, Venue, VenueKind};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;

use super::BscDex;
use super::{
    coins, connection, directory, exec, pools, quote, resolve, resolve_withdraw, send, wallet,
};
use crate::gateway::urgency::{self, Urgency as Class, scope};

#[async_trait]
impl Venue for BscDex {
    async fn trade_settlement_swap(
        &self,
        req: TradeRequest,
        quote: &Asset,
    ) -> GatewayResult<TradeResult> {
        scope(
            Class::Trade,
            exec::settlement_swap::trade(&self.p, req, quote),
        )
        .await
    }

    fn supports_settlement_withdrawal(&self) -> bool {
        let ctx = &self.p.ctx;
        let (Ok(chain), Ok(ticker)) = (ctx.text("dex.chain"), ctx.text("venue.quote_asset")) else {
            return false;
        };
        !chain.trim().is_empty()
            && !ticker.trim().is_empty()
            && super::node::chain_id::chain_id(ctx).is_ok_and(|id| id != 0)
            && crate::settings::evm_profiles::settlement_contract(
                &ctx.settings,
                ctx.place.id,
                &chain,
                &ticker,
            )
            .is_ok_and(|v| v.is_some())
    }

    async fn source_quote(
        &self,
        source: &str,
        req: crate::gateway::source_quote::Request,
    ) -> GatewayResult<crate::gateway::source_quote::Preview> {
        if source == "airswap" {return super::airswap::request(&self.p,req).await;}
        if source == "textile" {
            return super::textile::request(&self.p,req).await;
        }
        if req.buy_amount_raw.is_some() || req.firm.as_ref().is_some_and(|t|t.allow_trade || t.maximum_raw.is_some()) {
            return Err(self.p.ctx.err(ErrorCode::NotApplicable,"Этот источник не принимает условия заявки Textile"));
        }
        super::source_quote::preview(&self.p.ctx, source, req).await
    }

    /// Идентификатор площадки (`bsc_dex`). Ошибок нет.
    fn id(&self) -> VenueId {
        self.p.ctx.venue()
    }

    /// Вид площадки — DEX. Ошибок нет.
    fn kind(&self) -> VenueKind {
        VenueKind::Dex
    }

    /// Решение 352: эпоха котировки DEX (signal_epoch.rs). Ошибок нет.
    fn signal_epoch(&self, asset: &Asset) -> Option<String> {
        if super::airswap::routing::available(&self.p) || self.p.textile.has(asset) { return None; }
        super::signal_epoch::of(&self.p, asset)
    }

    /// Решение 372: строгий верх курса стороны DEX без котировок (quote/bound.rs). Ошибок нет.
    fn rate_bound(&self, asset: &Asset, side: Side) -> crate::gateway::rate_bound::RateBound {
        if super::airswap::routing::available(&self.p) || self.p.textile.has(asset) || super::hashflow::enabled(&self.p.ctx) || super::native_rfq::routing::available(&self.p,asset,side) || super::gladius::routing::available(&self.p,asset,side) || super::raindex::routing::available(&self.p,asset,side) || super::epsilon::routing::available(&self.p,asset)
            || super::hanji::routing::active(&self.p)
            || super::ryze::routing::active(&self.p)
            || super::fermi::routing::active(&self.p)
        {
            return crate::gateway::rate_bound::RateBound::na(
                "rfq",
                "Индикативные уровни RFQ не доказывают верх цены подписанной котировки",
            );
        }
        super::quote::bound::rate_bound(&self.p, asset, side)
    }

    /// Решение 132: вид адаптера на кошелёк счёта — venue_account.rs. Ошибки: NotConfigured — нет кошелька.
    fn for_account(&self, account: &DexAccountId) -> GatewayResult<std::sync::Arc<dyn Venue>> {
        self.on_account(account)
    }

    /// «Проверить подключение»: узел HTTP (eth_chainId = dex.evm_chain_id, решение 416; eth_blockNumber), узел WebSocket (newHeads), кошелёк,
    /// eth_simulateV1 — connection.rs (execution-apis src/eth, коммит 16caee27). Ошибок нет: причина — в ответе.
    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        Ok(connection::check(&self.p.ctx).await)
    }

    /// Монеты DEX с достоверными пулами (адрес токена — из пула, сеть BSC) — coins.rs. Ошибки: NotConfigured.
    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        coins::networks(&self.p.ctx, &self.p.registry)
    }

    async fn scaled_token_unit(
        &self,
        token: alloy_primitives::Address,
    ) -> GatewayResult<Option<Decimal>> {
        self.settlement_profile(token).await.map(|(_, _, m)| m)
    }

    async fn settlement_profile(
        &self,
        token: alloy_primitives::Address,
    ) -> GatewayResult<(u64, u8, Option<Decimal>)> {
        let block = self.p.heads.fresh(&self.p.ctx)?.number;
        let (decimals, scale) = self.p.scaled_units.read(&self.p.ctx, token, block).await?;
        Ok((block, decimals, scale))
    }

    async fn confirmed_ui_credit(
        &self,
        asset: &Asset,
        tx_hash: &str,
        raw_credited: Decimal,
    ) -> GatewayResult<Option<Decimal>> {
        self.p.ctx.bound()?;
        Ok(self
            .transfer_display_unit(asset, tx_hash, raw_credited, true)
            .await?
            .map(|(ui, _)| ui))
    }

    async fn transfer_display_unit(
        &self,
        asset: &Asset,
        hash: &str,
        raw: Decimal,
        incoming: bool,
    ) -> GatewayResult<Option<(Decimal, Decimal)>> {
        self.p.ctx.bound()?;
        wallet::scaled_credit::confirmed(&self.p, asset, hash, raw, incoming).await
    }

    /// Подписка на рынок: опрос блоков (решение 57), пулы монет (eth_subscribe logs, execution-apis subscribe.yaml), новые пулы,
    /// циклы проверок; события — в `out` (pools/market.rs). Прежние задачи подписки останавливаются.
    /// Решение 416: перед подпиской — сеть площадки (node/chain_id.rs check_start).
    /// Ошибки: NotConfigured (venue.quote_asset, dex.*, dex.chain, dex.evm_chain_id), BadResponse (узел другой сети), узел.
    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        // Решение 132: подписка — только у основного адаптера; вид на счёт рынок не слушает.
        if self.is_bound() {
            return Err(self.p.ctx.err(
                ErrorCode::NotApplicable,
                "подписка на рынок — у адаптера DEX без привязки к кошельку",
            ));
        }
        // Исполнению через KyberSwap (exec.via_kyber) нужен тот же набор монет (адрес USDT и монет) — отдаём его
        // контексту KS. KyberSwap как площадку-цены не подписываем: цены и сигналы идут с bsc_dex.
        self.subs.subscribe(&self.p, assets.clone(), out).await?;
        self.kyber.set_assets(assets);
        Ok(())
    }

    /// Котировка на объём по пулам (локальная математика по официальным библиотекам — quote/, math/).
    /// Ошибки: StaleData, CoinNotChecked / CoinCheckFailed, NoRoute, NotConfigured, узел.
    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        if super::quote::vista_pricing::cash_market(&self.p) {
            return scope(
                urgency::is_set().unwrap_or(Class::Quote),
                self.p.quoting().quote(asset, side, qty),
            )
            .await;
        }
        if crate::gateway::quote_stage::settlement() {
            let class = urgency::is_set().unwrap_or(Class::Quote);
            let baseline = scope(class, self.p.quoting().quote(asset, side, qty)).await;
            let baseline=super::raindex::routing::consider(&self.p,asset,side,qty,baseline).await;
        let baseline=super::epsilon::routing::consider(&self.p,asset,side,qty,baseline).await;
            let baseline=super::ryze::routing::consider(&self.p,asset,side,qty,baseline).await;
            return super::hanji::routing::consider(&self.p, asset, side, qty, baseline).await;
        }
        let baseline = async {
            // Решение 95: финальная котировка расчёта кандидата (ядро пересчитывает найденный оптимум) при exec.via_kyber —
            // цена KyberSwap, того, кто исполнит обмен; по ней ядро решает «торговать или нет». У KyberSwap нет маршрута
            // (пул, которого он не знает) — наша котировка: исполнит наш собственный сборщик (откат в trade()).
            if crate::gateway::quote_stage::is_final()
                && self.p.ctx.flag("exec.via_kyber").unwrap_or(false)
            {
                // Решение 123: «лучший из двух» — KyberSwap и своя параллельно, берётся лучшая (best_of/quote.rs).
                if super::best_of::enabled(&self.p.ctx) {
                    return super::best_of::quote::quote(&self.p, &self.kyber, asset, side, qty)
                        .await;
                }
                match crate::venues::kyberswap::quote::quote(&self.kyber, asset, side, qty).await {
                    Err(e) if e.code == ErrorCode::NoRoute => {}
                    Err(e) => return Err(e),
                    Ok(mut q) => {
                        // Котировка от имени площадки bsc_dex: ядро сверяет площадку стороны.
                        q.venue = self.p.ctx.venue();
                        return Ok(q);
                    }
                }
            }
            // Котировка: ядро на пути сделки ставит Trade (расчёт кандидата, предпроверка); иначе — Quote (живой экран,
            // ниже проб налогов, выше фона).
            let class = urgency::is_set().unwrap_or(Class::Quote);
            scope(class, self.p.quoting().quote(asset, side, qty)).await
        }
        .await;
        let baseline =
            super::hashflow::routing::consider(&self.p, asset, side, qty, baseline).await;
        let baseline=super::native_rfq::routing::consider(&self.p,asset,side,qty,baseline).await;
        let baseline=super::raindex::routing::consider(&self.p,asset,side,qty,baseline).await;
        let baseline=super::epsilon::routing::consider(&self.p,asset,side,qty,baseline).await;
        let baseline=super::airswap::routing::consider(&self.p,asset,side,qty,baseline).await;
        let baseline=super::textile::routing::consider(&self.p,asset,side,qty,baseline).await;
        let baseline=super::gladius::routing::consider(&self.p,asset,side,qty,baseline).await;
        let baseline=super::ryze::routing::consider(&self.p,asset,side,qty,baseline).await;
        let baseline = super::fermi::routing::consider(&self.p, asset, side, qty, baseline).await;
        super::hanji::routing::consider(&self.p, asset, side, qty, baseline).await
    }

    /// Решение 250: исполнимость объёма (проверка доли глубины h) — quote/fills.rs: прямой путь одного пула исполняет
    /// объём целиком — ответ без полной котировки; иначе — полная котировка, как quote. Финальная котировка через
    /// KyberSwap (решение 95) — как quote. Ошибки: как у quote.
    async fn quote_fills(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Decimal> {
        if super::quote::vista_pricing::cash_market(&self.p) {
            return self.quote(asset, side, qty).await.map(|q| q.fillable_qty);
        }
        if crate::gateway::quote_stage::settlement() {
            return self.quote(asset, side, qty).await.map(|q| q.fillable_qty);
        }
        if super::airswap::routing::available(&self.p) || self.p.textile.has(asset) || super::hashflow::enabled(&self.p.ctx) || super::native_rfq::routing::available(&self.p,asset,side) || super::gladius::routing::available(&self.p,asset,side) || super::raindex::routing::available(&self.p,asset,side) || super::epsilon::routing::available(&self.p,asset)
            || super::hanji::routing::active(&self.p)
            || super::ryze::routing::active(&self.p)
            || super::fermi::routing::active(&self.p)
            || (crate::gateway::quote_stage::is_final()
                && self.p.ctx.flag("exec.via_kyber").unwrap_or(false))
        {
            return self.quote(asset, side, qty).await.map(|q| q.fillable_qty);
        }
        let class = urgency::is_set().unwrap_or(Class::Quote);
        scope(class, self.p.quoting().fills(asset, side, qty)).await
    }

    /// Решение 344 (задача t159, ревью t159 Б4): изломы кривых пулов маршрута котировки по объёму монеты стороны —
    /// quote/breaks.rs (только память реестра). Ошибки: NoRoute (нет адреса монеты), NotConfigured (тикер USDT), узел
    /// (decimals).
    async fn quote_breaks(
        &self,
        asset: &Asset,
        side: Side,
        route: &DexRoute,
        max_qty: Decimal,
        limit: u32,
    ) -> GatewayResult<Vec<Decimal>> {
        let q = self.p.quoting();
        let mut extra = super::hanji::routing::breaks(&self.p, asset, side, max_qty);
        extra.extend(self.p.ryze.breaks(&self.p,asset,side,max_qty));
        extra.extend(super::raindex::routing::breaks(&self.p,asset,side,max_qty));
        if !crate::gateway::quote_stage::settlement() {
            extra.extend(self.p.airswap.breaks(&self.p,asset,side,max_qty));
            extra.extend(self.p.textile.breaks(&self.p,asset,side,max_qty));
            extra.extend(super::hashflow::routing::breaks(
                &self.p, asset, side, max_qty,
            ));
            extra.extend(super::fermi::routing::breaks(&self.p, asset, side, max_qty));
            extra.extend(super::native_rfq::routing::breaks(&self.p, asset, side, max_qty));
            extra.extend(super::gladius::routing::breaks(&self.p, asset, side, max_qty));
        }
        if extra.is_empty() {
            return super::quote::breaks::breaks(&q, asset, side, route, max_qty, limit).await;
        }
        let mut points = if matches!(
            route.mode,
            crate::core::types::RouteMode::RfqIndicative
                | crate::core::types::RouteMode::Relay(_)
                | crate::core::types::RouteMode::RelayIndicative
                | crate::core::types::RouteMode::Rfq(_)
                | crate::core::types::RouteMode::Orderbook(_)
                | crate::core::types::RouteMode::Slot(_)
                | crate::core::types::RouteMode::OrderbookIndicative
                | crate::core::types::RouteMode::SlotIndicative
        ) {
            vec![]
        } else {
            super::quote::breaks::breaks(&q, asset, side, route, max_qty, limit).await?
        };
        points.extend(extra);
        points.sort_unstable();
        points.dedup();
        points.truncate(limit as usize);
        Ok(points)
    }

    /// Решение 217: снимок котировки на момент решения — `Arc` записей пулов, разрешения, блок (frozen.rs). Только
    /// память. Ошибки: StaleData (блок, лента событий), NotConfigured (тикер USDT), NoRoute (нет адреса монеты).
    async fn quote_snapshot(
        &self,
        asset: &Asset,
        side: Side,
    ) -> GatewayResult<crate::gateway::frozen::QuoteSnapshot> {
        Ok(std::sync::Arc::new(super::frozen::take(
            self.p.quoting(),
            asset,
            side,
        )?))
    }

    /// Обмен через Universal Router своей DEX (прямой) или PancakeSwap Infinity (умный) — exec/trade.rs; источники —
    /// research/flow/bsc_dex.md, раздел 5, bsc_smart_router.md. Ошибки: см. exec/trade.rs (с фактом транзакции).
    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        let asked = req.clone();
        // Решение 222: монета сделки — по изменению баланса кошелька на блоке свопа (все пути исполнения ниже).
        let r = self.trade_logs(req).await;
        let fact = exec::balance_fact::apply(&self.p.ctx, &self.p.tokens, r);
        // Решение 231: «исход неизвестен» с фактом транзакции — «чем найти» (хеш, nonce) для ядра (resolve.rs).
        let r = scope(Class::Trade, fact).await.map_err(resolve::attach);
        // t78: продажа выхода сделки исполнилась целиком — её налоги кошелька сделки больше не нужны (deal_taxes.rs).
        self.p.checks.forget_after_sale(&asked, &r);
        match r {
            Ok(t) => super::hashflow::record(&self.p, t).await,
            Err(e) => Err(super::hashflow::failed(&self.p, &asked, e).await),
        }
    }

    /// Перевод монеты из кошелька на адрес (ERC-20 transfer, EIP-20) — wallet/transfer.rs.
    /// Ошибки: InvalidAddress, NotConfigured, каналы, TxNotIncluded / WithdrawRejected (с фактом транзакции).
    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        self.p.ctx.bound()?;
        let env = self.p.env();
        let asked = req.clone();
        let r = scope(
            Class::Trade,
            wallet::transfer::withdraw(&env, &self.p.tokens, &self.p.nonces, req),
        )
        .await
        .map_err(resolve::attach);
        // t78: монета сделки ушла с кошелька — её налоги кошелька сделки больше не нужны (deal_taxes.rs).
        self.p.checks.forget_after_withdraw(&asked, &r);
        r
    }

    /// Статус перевода из кошелька по квитанции (execution-apis transaction.yaml#L127-L165) — wallet/transfer_status.rs.
    /// Ошибки: узел, хеш не 32 байта, BadResponse.
    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        self.p.ctx.bound()?;
        let w = wallet::signer::address(&self.p.ctx)?;
        let work =
            wallet::transfer_status::withdrawal_status(&self.p.ctx, &self.p.tokens, venue_ref, w);
        scope(Class::Trade, work).await
    }

    /// Решение 231: своп с неизвестным исходом — квитанция по хешу; очередь кошелька снимается, итог свопа — по
    /// контексту разбора из «чем найти» (resolve.rs, exec/swap_rebuild.rs; нет контекста — человек); монета сделки — по
    /// балансу кошелька на блоке свопа (решение 222, как у trade).
    async fn resolve_trade(
        &self,
        req: TradeRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<TradeResult>> {
        self.p.ctx.bound()?;
        let work = async {
            let resolved = match resolve::trade(&self.p, req.clone(), p).await {
                Ok(r) => r,
                Err(e) => return Err(super::hashflow::failed(&self.p, &req, e).await),
            };
            match resolved {
                Some(t) => {
                    let t = exec::balance_fact::apply(&self.p.ctx, &self.p.tokens, Ok(t)).await?;
                    super::hashflow::record(&self.p, t).await.map(Some)
                }
                None => Ok(None),
            }
        };
        scope(Class::Trade, work).await
    }

    /// Решение 231: перевод из кошелька с неизвестным исходом — квитанция по хешу; хеши отмен известны — и их квитанции
    /// и nonce кошелька (resolve_withdraw.rs).
    async fn resolve_withdraw(
        &self,
        req: WithdrawRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<Transfer>> {
        self.p.ctx.bound()?;
        scope(Class::Trade, resolve_withdraw::withdraw(&self.p, req, p)).await
    }

    /// Адрес кошелька; заодно регистрирует ожидаемое поступление монеты (после конца идущего обмена с ней на выходе) —
    /// wallet/inflows.rs. Ошибки: ключ кошелька, NotConfigured (у монеты нет адреса в BSC).
    async fn deposit_address(&self, asset: &Asset, _network: &str) -> GatewayResult<String> {
        // Решение 132: адрес депозита — кошелька счёта сделки; без счёта не отдаётся (монеты не уйдут не туда).
        self.p.ctx.bound()?;
        let w = wallet::signer::address(&self.p.ctx)?;
        let token = wallet::transfer::token(&self.p.ctx, asset)?;
        self.p.inflows.expect(token).await;
        Ok(w.to_string())
    }

    /// Статус зачисления; любое завершение снимает ожидаемое поступление — wallet/deposit.rs.
    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        self.p.ctx.bound()?;
        scope(
            Class::Trade,
            wallet::deposit::status(&self.p, asset, tx_hash),
        )
        .await
    }

    fn needs_deposit_claim(&self, asset: &Asset) -> bool {
        !super::exec::native::is_native(&self.p.ctx, asset)
    }

    async fn deposit_status_for(
        &self,
        asset: &Asset,
        tx: &str,
        claim: &crate::gateway::contract::DepositClaim,
        event_amount: Option<Decimal>,
    ) -> GatewayResult<Transfer> {
        self.p.ctx.bound()?;
        scope(
            Class::Trade,
            wallet::deposit::status_for(&self.p, asset, tx, Some(claim), event_amount),
        )
        .await
    }

    /// Вывод на кошелёк не состоялся — ожидание поступления снято (wallet/deposit.rs). Ошибок нет.
    async fn deposit_abandoned(&self, asset: &Asset) -> GatewayResult<()> {
        self.p.ctx.bound()?;
        wallet::deposit::abandoned(&self.p, asset).await;
        Ok(())
    }

    /// Адрес кошелька счёта (решение 132) — wallet/signer.rs; только у вида на счёт. Ошибки: счёт не выбран, ключ.
    fn settlement_account(&self) -> GatewayResult<String> {
        self.p.ctx.bound()?;
        Ok(wallet::signer::address(&self.p.ctx)?.to_string())
    }
    async fn wallet_address(&self) -> GatewayResult<String> {
        self.p.ctx.bound()?;
        Ok(wallet::signer::address(&self.p.ctx)?.to_string())
    }

    /// Очередь транзакций кошелька (wallet/nonce.rs). Ошибок нет.
    async fn wallet_queue(&self) -> GatewayResult<WalletQueue> {
        self.p.ctx.bound()?;
        Ok(wallet::deposit::queue(&self.p).await)
    }

    /// «Снять очередь после ручной проверки» — wallet/deposit.rs. Ошибки: NotApplicable — очередь не остановлена.
    async fn release_wallet_queue(&self, by: &str, pending_ok: bool) -> GatewayResult<WalletQueue> {
        self.p.ctx.bound()?;
        scope(
            Class::Trade,
            wallet::deposit::release(&self.p, by, pending_ok),
        )
        .await
    }

    /// Решение 437 (3): доля прибыли на чаевые сделок DEX ↔ DEX площадки — exec/tip_state.rs. Ошибки: NotApplicable
    /// — площадка не dex.gas_mode = eip1559_l1 (ревью 437, п. 6).
    async fn tip_share(&self) -> GatewayResult<crate::core::types::TipShare> {
        exec::tip_state::view(&self.p.ctx).await
    }

    /// Решение 437 (3): «Сбросить долю чаевых» к минимуму — exec/tip_state.rs. Ошибки: NotApplicable (не eip1559_l1).
    async fn reset_tip_share(&self, by: &str) -> GatewayResult<crate::core::types::TipShare> {
        exec::tip_state::reset(&self.p.ctx, by).await
    }

    /// Решение 439: статистика условия пулов площадки — exec/pool_cond_stat.rs. Ошибки: NotApplicable (не eip1559_l1).
    async fn pool_cond_stat(&self) -> GatewayResult<crate::core::types::PoolCondStat> {
        exec::pool_cond_stat::view(&self.p.ctx).await
    }

    /// Решение 439: «Сбросить статистику» условия пулов — exec/pool_cond_stat.rs. Ошибки: NotApplicable.
    async fn reset_pool_cond_stat(
        &self,
        by: &str,
    ) -> GatewayResult<crate::core::types::PoolCondStat> {
        exec::pool_cond_stat::reset(&self.p.ctx, by).await
    }

    /// Балансы кошелька: BNB (eth_getBalance) и известные токены (balanceOf, EIP-20) — wallet/balances.rs. Ошибки: узел, ключ.
    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        self.p.ctx.bound()?;
        wallet::balances::all(&self.p.ctx, &self.p.tokens).await
    }

    /// Баланс одного актива: 1–2 запроса к узлу вместо запроса по каждому токену (решение 113) — wallet/balances.rs.
    /// Ошибки: ключ кошелька, узел.
    async fn balance(&self, asset: &str) -> GatewayResult<Option<Balance>> {
        self.p.ctx.bound()?;
        wallet::balances::single(&self.p.ctx, &self.p.tokens, asset).await
    }

    /// Стоимость перевода: газ (пробный перевод × цена газа) и налог на перевод — wallet/transfer.rs.
    /// Ошибки: CoinNotChecked / CoinCheckFailed, узел.
    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        let env = self.p.env();
        scope(
            Class::Trade,
            wallet::transfer::cost(&env, &self.p.checks, asset, network, amount),
        )
        .await
    }

    /// Кнопка «Перепроверить»: налоги, крючки и сверка с квотером — только этих монет (check/recheck.rs; итоги —
    /// события CoinCheck / PoolCheck). Ошибок нет.
    async fn recheck_coins(&self, assets: Vec<Asset>) -> GatewayResult<()> {
        // Решение 211: монета — адрес контракта (у монеты без адреса пулов на DEX нет — проверять нечего).
        let coins: Vec<alloy_primitives::Address> =
            assets.into_iter().filter_map(|a| a.contract).collect();
        self.p.wake.coins(&coins);
        Ok(())
    }

    /// DEX с видом пулов из таблицы сопоставления (id области «DEX» = id DEX индексатора) — pools/families.rs; решение
    /// 128 — плюс DEX из справочника пулов и id с уже заданными настройками (вид None) — pools/dex_ids.rs. Ошибок нет.
    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        Ok(pools::families::for_ui(&self.p.ctx))
    }

    /// Решение 308: состояние фонового владельца графа маршрутов для экрана «Площадки» — pools/graph_stats.rs. Ошибок нет.
    async fn dex_graph_state(&self) -> GatewayResult<crate::core::types::DexGraphState> {
        Ok(pools::graph_stats::view(&self.p.ctx, &self.p.registry))
    }

    /// Решение 308: полная пересборка графа маршрутов со сверкой по команде (кнопка «Пересобрать граф», REST) — действующий
    /// снимок работает до замены. Ошибки: NotConfigured — dex.graph_background не «да»; VenueNotConnected — DEX не
    /// подписан на рынок (фонового владельца нет).
    async fn dex_graph_rebuild(&self, by: &str) -> GatewayResult<()> {
        let p = &self.p;
        if !pools::graph_read::background(&p.ctx) {
            return Err(p.ctx.not_set(
                "dex.graph_background",
                "Пересборка графа: dex.graph_background не включена — граф строит расчёт, как раньше",
            ));
        }
        let bg = p.registry.graph_cache().bg();
        if !bg.started() {
            return Err(p.ctx.err(
                ErrorCode::VenueNotConnected,
                "Пересборка графа: DEX не подписан на рынок — фоновой задачи графа нет",
            ));
        }
        bg.request_full(format!("команда «Пересобрать граф» ({by})"));
        pools::graph_alarm::info(
            &p.ctx,
            bg,
            format!("Запрошена полная пересборка графа маршрутов со сверкой ({by})"),
        );
        Ok(())
    }

    /// Справочник пулов от индексатора (решение 58) — directory/. Ошибок нет: нет данных — пустые списки и причина.
    async fn pool_directory(&self) -> GatewayResult<PoolDirectory> {
        Ok(directory::snapshot(&self.p.ctx))
    }

    /// Пулы для экрана «Монеты» (решение 73) — pools/depth_facts.rs. Ошибки: USDT не известен, узел.
    async fn pool_depths(&self) -> GatewayResult<Vec<crate::core::types::PoolDepth>> {
        let p = &self.p;
        pools::depth_facts::all(&p.ctx, &p.registry, &p.tokens, &p.checks, None).await
    }

    /// Только перечисленные пулы (решение 74: пулы маршрута в карточке сигнала) — pools/depth_facts.rs.
    /// Ошибки: USDT не известен, узел.
    async fn pool_depths_of(
        &self,
        pools: &[crate::core::types::PoolRef],
    ) -> GatewayResult<Vec<crate::core::types::PoolDepth>> {
        let p = &self.p;
        pools::depth_facts::all(&p.ctx, &p.registry, &p.tokens, &p.checks, Some(pools)).await
    }

    /// Сигналы циклов DEX ↔ DEX (решение 85, фаза сигнала): поиск циклов (quote/cycle_scan.rs) и перевод в read-тип ядра
    /// (quote/cycle_view.rs). Направление за dex_dex.enabled; выключено — пусто (газ не трогаем). Только read-поверхность:
    /// сделок не запускает (их ведёт ядро, решение 124). Ошибки: StaleData (нет свежего блока), NotConfigured (настройки цикла), узел.
    async fn dex_cycles(&self) -> GatewayResult<Vec<crate::core::types::DexCycle>> {
        let q = self.p.quoting();
        if !quote::cycle_scan::enabled(&q.ctx) {
            return Ok(Vec::new());
        }
        // Цена газа — как при котировке и отправке: eth_gasPrice с надбавкой, не ниже минимума готовых каналов.
        let fees = q.gas.for_quote(&q.ctx, &q.heads).await?;
        let gas = fees
            .wei(&q.ctx, &q.heads)
            .await?
            .max(send::planned_floor(&q.ctx)?);
        // Решение 131: поиск циклов — счёт CPU, вне рабочих потоков tokio (quote/cpu.rs).
        // Ревью 273: пулы Solidly с пределом цены газа фабрики ниже этой цены — не в циклы (quote/select.rs plan_gas).
        let mut qc = q.clone();
        qc.gas = fees;
        qc.plan_gas = Some(gas);
        // Решение 301 (дополнение — стек): работа — в кучу (quote/cpu.rs run принимает только Pin<Box<F>>).
        let found = quote::cpu::run(
            &q.ctx,
            Box::pin(async move { quote::cycle_scan::scan(&qc, gas).await }),
        )
        .await;
        // Решение 385: нарушения контроля отсева циклов — в журнал и Telegram (quote/cycle_audit.rs), и при сбое поиска.
        quote::cycle_audit::flush(&q.ctx, &self.p.stats).await;
        let signals = found?;
        let venue = self.id();
        Ok(signals
            .iter()
            .map(|s| quote::cycle_view::to_dex_cycle(&venue, s))
            .collect())
    }

    /// Решение 404: итог последнего прохода поиска циклов по базовым монетам (quote/cycle_tally.rs). Ошибок нет.
    fn dex_cycle_tally(&self) -> Vec<(String, serde_json::Value)> {
        quote::cycle_tally::last(&self.id().0)
    }

    /// «Сухая сделка DEX» (решение 121, C4): сборка своим сборщиком как для сделки и eth_simulateV1 от кошелька —
    /// exec/dry_swap.rs. Ничего не отправляет, nonce не занимает. Ошибок нет: отказ — причиной в итоге.
    async fn dex_dry_swap(
        &self,
        asset: Asset,
        side: Side,
        usdt: Decimal,
    ) -> GatewayResult<crate::core::types::DexDrySwap> {
        let q = self.p.quoting();
        // Класс Check (решение 61): ниже пути сделки, выше котировок живого экрана и фона.
        let mode = exec::executor::dry::DryMode::AsTrade;
        Ok(scope(
            Class::Check,
            exec::dry_swap::run(&q, asset, side, usdt, mode),
        )
        .await)
    }

    /// Решение 234: симуляция продажи до покупки — тем путём, кошельком счёта и минимумом, что у настоящей продажи
    /// (exec/presell.rs). Класс — как у котировки ядра (предпроверка сделки), иначе Check. Ничего не отправляет.
    /// Ошибки: см. exec/presell.rs.
    async fn dex_presell(
        &self,
        req: &TradeRequest,
    ) -> GatewayResult<crate::core::types::PresellOutcome> {
        let q = self.p.quoting();
        let class = urgency::is_set().unwrap_or(Class::Check);
        scope(class, exec::presell::run(&q, req.clone())).await
    }

    /// Решение 476: разрешения (approve) продажи заранее, пока монета идёт на кошелёк сделки — exec/preapprove.rs (сборка
    /// плана как у исполнения, wallet/approve.rs lacking, отправка exec/swap_send.rs send_approvals). Ошибки: как у run.
    /// Ревью t238: путь исполнения продажи не свой сборщик (exec.via_kyber: KyberSwap / «лучший из двух»,
    /// trade_path.rs — как в trade_logs) — разрешение получит другой: NotApplicable с причиной.
    async fn dex_preapprove(
        &self,
        req: &TradeRequest,
        horizon_s: u64,
    ) -> GatewayResult<Option<Decimal>> {
        if matches!(
            req.route.as_ref().map(|r| r.mode),
            Some(
                crate::core::types::RouteMode::Rfq(_)
                    | crate::core::types::RouteMode::Orderbook(_)
                    | crate::core::types::RouteMode::Slot(_)
            )
        ) {
            return Err(self.p.ctx.err(
                ErrorCode::NotApplicable,
                "Внешний исполнитель требует ранее выданное разрешение кошелька",
            ));
        }
        let via_kyber = self.p.ctx.flag("exec.via_kyber").unwrap_or(false);
        let path = super::trade_path::of(req, via_kyber, super::best_of::enabled(&self.p.ctx));
        if path != super::trade_path::Path::Own {
            let why = "продажа не своим сборщиком (exec.via_kyber) — разрешение получит другой";
            return Err(self.p.ctx.err(ErrorCode::NotApplicable, why));
        }
        let (q, env) = (self.p.quoting(), self.p.env());
        let run = exec::preapprove::run(&q, &env, &self.p.nonces, req.clone(), horizon_s);
        scope(Class::Trade, run).await
    }

    /// «Сухая сделка DEX» через исполнитель ArbExecutor с подставленным в симуляцию кодом (решение 122, часть X) —
    /// проверка исполнителя на живой сети до развёртывания. Ничего не отправляет. Ошибок нет: отказ — причиной в итоге.
    async fn dex_dry_swap_executor(
        &self,
        asset: Asset,
        side: Side,
        usdt: Decimal,
    ) -> GatewayResult<crate::core::types::DexDrySwap> {
        let q = self.p.quoting();
        let mode = exec::executor::dry::DryMode::Executor;
        Ok(scope(
            Class::Check,
            exec::dry_swap::run(&q, asset, side, usdt, mode),
        )
        .await)
    }

    /// Расчёт цикла DEX ↔ DEX на вход (решение 124): ноги по памяти реестра (quote/cycle_legs.rs), курсы и затраты —
    /// quote/cycle_cost.rs; with_wallet — ещё баланс базовой у кошелька. Ошибки: NoRoute, NotConfigured, узел.
    async fn dex_cycle_quote(
        &self,
        cycle: &crate::core::types::DexCycle,
        input: Decimal,
        with_wallet: bool,
    ) -> GatewayResult<crate::core::types::CycleQuote> {
        // Баланс базовой — кошелька счёта циклов (решение 132).
        if with_wallet {
            self.p.ctx.bound()?;
        }
        let q = self.p.quoting();
        // Решение 336: точки подбора объёма ядра считаются параллельно (gateway/calc_share.rs par) — расчёт цикла
        // уходит в поток счёта (quote/cpu.rs), иначе одновременные точки шли бы одним рабочим потоком. Иначе — как раньше.
        if crate::gateway::calc_share::current().is_some_and(|s| s.par()) {
            let ctx = q.ctx.clone();
            let cycle = cycle.clone();
            let run = async move {
                let legs = quote::cycle_legs::now(&q, &cycle).await?;
                let r = quote::cycle_cost::rates(&q, cycle.base).await?;
                quote::cycle_cost::quote(&q, &legs, cycle.base, input, (r, with_wallet)).await
            };
            return scope(Class::Quote, async {
                quote::cpu::run(&ctx, Box::pin(run)).await
            })
            .await;
        }
        let run = async {
            let legs = quote::cycle_legs::now(&q, cycle).await?;
            let r = quote::cycle_cost::rates(&q, cycle.base).await?;
            quote::cycle_cost::quote(&q, &legs, cycle.base, input, (r, with_wallet)).await
        };
        scope(Class::Quote, run).await
    }

    /// Сделка цикла DEX ↔ DEX (решение 124) — exec/cycle_trade.rs: свежее перечтение, исполнитель или бандл, проба,
    /// одна отправка без повторов. Ошибки: как у сделки DEX.
    async fn dex_cycle_trade(
        &self,
        req: crate::core::types::CycleTradeRequest,
    ) -> GatewayResult<crate::core::types::CycleTradeResult> {
        self.p.ctx.bound()?;
        let (q, env) = (self.p.quoting(), self.p.env());
        scope(
            Class::Trade,
            exec::cycle_trade::trade(&q, &env, &self.p.nonces, req),
        )
        .await
    }

    /// Решение 233 (дополнение, п. 4): цикл DEX ↔ DEX с неизвестным исходом — тем же механизмом, что своп (resolve.rs:
    /// квитанция / бандл — resolve_bundle.rs; очередь кошелька снимается при выясненном исходе), итог цикла — с курсами
    /// сделки из «чем найти» (resolve_cycle.rs). Решение 222 не применяется — как у dex_cycle_trade.
    async fn resolve_cycle(
        &self,
        req: crate::core::types::CycleTradeRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<crate::core::types::CycleTradeResult>> {
        self.p.ctx.bound()?;
        scope(Class::Trade, super::resolve_cycle::cycle(&self.p, req, p)).await
    }

    /// Сухая сделка цикла (решение 124) — exec/cycle_dry.rs: сборка как у сделки и eth_simulateV1, ничего не уходит.
    /// Ошибок нет: отказ — причиной в итоге.
    async fn dex_dry_cycle(
        &self,
        req: crate::core::types::CycleTradeRequest,
        executor: bool,
    ) -> GatewayResult<crate::core::types::DexDrySwap> {
        let q = self.p.quoting();
        let mode = if executor {
            exec::executor::dry::DryMode::Executor
        } else {
            exec::executor::dry::DryMode::AsTrade
        };
        Ok(scope(Class::Check, exec::cycle_dry::run(&q, req, mode)).await)
    }

    /// Оценка развёртывания исполнителя ArbExecutor (решение 122, часть X): газ, цена, стоимость, будущий адрес — ничего не
    /// отправляет. Ошибки: ключ кошелька, каналы, узел.
    async fn dex_executor_quote(&self) -> GatewayResult<crate::core::types::ExecutorDeployQuote> {
        // Решение 132: исполнитель разворачивается кошельком, у которого нажата кнопка.
        self.p.ctx.bound()?;
        let (q, env) = (self.p.quoting(), self.p.env());
        scope(Class::Check, exec::executor::deploy::quote(&q, &env)).await
    }

    /// Решение 399: причина запрета развёртывания исполнителя (запись приёмки версии текущей сборки в сети площадки —
    /// решение 461 — не приняла ни одного класса или её нет; exec/executor/deploy_gate.rs). None — разрешено. Ошибок нет.
    fn dex_executor_refusal(&self) -> Option<String> {
        let chain = exec::executor::live_proof::chain_of(&self.p.ctx);
        exec::executor::deploy_gate::refusal_at(chain)
    }

    /// Решение 400: классы шага исполнителя v3 и их приёмка по записи симуляции на живых пулах
    /// (exec/executor/live_proof.rs); решение 401 — сводка пробы пулов через исполнитель кошелька окружения
    /// (exec/executor/pass_view.rs). Ошибок нет.
    fn dex_executor_classes(&self) -> Vec<crate::core::types::ExecutorClassProof> {
        exec::executor::pass_view::proofs(&self.p.ctx, &self.p.registry)
    }

    /// Развёртывание исполнителя ArbExecutor с кошелька бота (решение 122, часть X) — только по подтверждению человека:
    /// SHA-256 байткода и адрес обёртки (задача t114, 3в), которые он видел в оценке. Ошибки: QuoteMismatch (байткод или
    /// обёртка не те), ключ, каналы, узел.
    async fn dex_executor_deploy(
        &self,
        sha256: String,
        wnative: alloy_primitives::Address,
    ) -> GatewayResult<crate::core::types::ExecutorDeployed> {
        self.p.ctx.bound()?;
        let (q, env) = (self.p.quoting(), self.p.env());
        let run = exec::executor::deploy::deploy(&q, &env, &self.p.nonces, (&sha256, wnative));
        scope(Class::Trade, run).await
    }

    /// «Проверить подключение» исполнителя ArbExecutor (ревью 122) — exec/executor/onchain.rs. Её же зовут после
    /// развёртывания и «Записать после проверки». Ревью 272: v1 — уведомление один раз (Б5, v1_notice.rs); кошелёк
    /// строки «Пробы и сигналы» теперь годен для BNB через исполнитель — внеочередная перепроверка монеты BNB (та же,
    /// что «Перепроверить», POST /api/coins/recheck): проба её пулов перемеряет газ обёртки через исполнитель (Б3), не
    /// дожидаясь планового прохода tax.recheck_interval_s. Ошибок нет.
    async fn check_executor(&self) -> GatewayResult<ConnectionCheck> {
        let ctx = &self.p.ctx;
        let c = exec::executor::onchain::probe(ctx).await;
        let Ok(w) = ctx.wallet() else { return Ok(c) };
        if let Some(v) = w
            .executor_version
            .as_deref()
            .and_then(exec::executor::code::Version::parse)
        {
            exec::executor::v1_notice::once(ctx, &self.p.stats, v).await;
        }
        let probe_row = ctx
            .settings
            .accounts()
            .row(crate::settings::accounts::ROW_PROBE);
        if c.ok
            && probe_row.as_deref() == Some(w.id.as_str())
            && exec::executor::native_capable(ctx)
            && let Some((ticker, wbnb)) = exec::native::wrapped(ctx)
        {
            self.p.wake.coins(&[wbnb]);
            let text = format!(
                "исполнитель кошелька проб «{}» сверен (v2 / v3, обёртка = dex.native_wrapped) — внеочередная перепроверка {ticker}: \
                 газ обёртки через исполнитель перемеряется пробой пулов",
                w.name
            );
            ctx.log(
                crate::storage::journal::Level::Info,
                "dex.native_probe",
                text,
                None,
            )
            .await;
        }
        Ok(c)
    }

    /// Каналы отправки (id области «канал») — send/channels.rs. Ошибок нет.
    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        Ok(send::channels::ALL
            .iter()
            .filter(|ch| send::channels::compatible_ctx(&self.p.ctx, ch))
            .map(|c| c.info())
            .collect())
    }

    /// «Проверить подключение» канала: пустая транзакция — канал отвечает ошибкой JSON-RPC (send/check.rs; документация
    /// канала — в шапке его файла). Ошибки: неизвестный id.
    async fn check_send_channel(&self, id: &str) -> GatewayResult<ConnectionCheck> {
        send::check::check(&self.p.ctx, id).await
    }
}

impl BscDex {
    /// Сделка по пути исполнения (свой сборщик, KyberSwap, «лучший из двух»); факт — по событиям Transfer квитанции.
    /// Ошибки: см. exec/trade.rs (с фактом транзакции).
    async fn trade_logs(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        // Решение 132: сделка — только кошельком счёта (Ctx::bound).
        self.p.ctx.bound()?;
        let mut req = req;
        if req.route.as_ref().is_some_and(|r| super::raindex::routing::mixed_ticket(r).is_some() || super::epsilon::routing::mixed_ticket(r).is_some()) {
            return Err(self.p.ctx.err(ErrorCode::NoRoute,"Денежный билет требует исходного расчётного шага"));
        }
        if let Some(done)=super::ryze::recovery::prior(&self.p,&req).await? {return Ok(done);}
        if !matches!(
            req.route.as_ref().map(|r| r.mode),
            Some(crate::core::types::RouteMode::Orderbook(_))
        ) && let Some(done) = super::hanji::execute::prior(&self.p, &req).await?
        {
            return Ok(done);
        }
        if matches!(
            req.route.as_ref().map(|r| r.mode),
            Some(
                crate::core::types::RouteMode::RfqIndicative
                    | crate::core::types::RouteMode::RelayIndicative
                    | crate::core::types::RouteMode::OrderbookIndicative
                    | crate::core::types::RouteMode::SlotIndicative
            )
        ) {
            let firm =
                crate::gateway::quote_stage::final_scope(self.quote(&req.asset, req.side, req.qty))
                    .await?;
            super::best_of::choose::within_worst(&firm, req.worst_price)
                .map_err(|why| self.p.ctx.err(ErrorCode::QuoteMismatch, why).price_moved())?;
            if firm.fillable_qty != req.qty || firm.route.as_ref().is_some_and(|r| !r.executable) {
                return Err(self.p.ctx.err(
                    ErrorCode::NoRoute,
                    "Нет подписанной котировки или обычного маршрута на весь объём",
                ));
            }
            req.route = firm.route;
        }
        if let Some(crate::core::types::RouteMode::Orderbook(id)) =
            req.route.as_ref().map(|r| r.mode)
        {
            return super::hanji::execute::trade(&self.p, req, id).await;
        }
        if let Some(crate::core::types::RouteMode::Relay(id))=req.route.as_ref().map(|r|r.mode) {return super::ryze::execute::trade(&self.p,req,id).await;}
        if let Some(crate::core::types::RouteMode::Slot(id)) = req.route.as_ref().map(|r| r.mode) {
            return super::fermi::trade(&self.p, req, id).await;
        }
        if let Some(crate::core::types::RouteMode::Rfq(id)) = req.route.as_ref().map(|r| r.mode) {
            return super::hashflow::routing::trade(&self.p, req, id).await;
        }
        // Исполнение через агрегатор KyberSwap (exec.via_kyber, решение 91): маршрут и своп строит KyberSwap по
        // объёму из расчёта (req.qty, req.worst_price); сигналы, цены и подбор размера — на bsc_dex. Нет маршрута у
        // KyberSwap (пул, которого он не знает) — откат на собственный исполнитель bsc_dex по маршруту котировки.
        // Решение 247, п. Г: продажа выхода с условиями пере-замера (повтор режима застрявшей монеты, кнопка человека —
        // TradeRequest.remeasure) — своим сборщиком, не KyberSwap: у своего пути проба перед отправкой и пере-замер
        // выхода при откате по minOut (exec/executor/remeasure.rs). t81 (Тигран 10.11: согласовано): пути t78 (признак
        // Remeasure.any_executor) — как до t78; у своего сборщика при этом пере-замер остаётся (trade_path.rs).
        let via_kyber = self.p.ctx.flag("exec.via_kyber").unwrap_or(false);
        let marked = req.route.as_ref().is_some_and(|r| r.legs.iter().flat_map(|l| &l.hops)
            .any(|h| super::quote::vista_pricing::cash_marker(h.fee_mode)));
        let cash_market = super::quote::vista_pricing::cash_market(&self.p);
        let path = if cash_market || marked {
            let selected =
                super::quote::vista_pricing::terminal_cash(&self.p.registry, req.route.as_ref());
            if !selected || !cash_market || !marked {
                return Err(self.p.ctx.err(
                    ErrorCode::NoRoute,
                    "Изменился выбранный кошельком путь расчёта в ETH",
                ));
            }
            super::trade_path::Path::Own
        } else {
            super::trade_path::of(&req, via_kyber, super::best_of::enabled(&self.p.ctx))
        };
        // Решение 123: «лучший из двух» — маршрут KyberSwap и свой в один момент, исполняет лучший (best_of/trade.rs).
        if path == super::trade_path::Path::BestOf {
            return super::best_of::trade::trade(&self.p, &self.kyber, req).await;
        }
        if path == super::trade_path::Path::Kyber {
            let kyber_result = async {
                let _slot = self.p.nonces.acquire(&self.p.ctx).await?;
                let prepared =
                    crate::venues::kyberswap::trade_prep::prepare(&self.kyber, &req).await?;
                crate::venues::kyberswap::trade::execute(&self.kyber, req.clone(), prepared).await
            }
            .await;
            match kyber_result {
                Err(e) if e.code == ErrorCode::NoRoute => {}
                // Решение 121 (B7): исполнитель — в тексте ошибки, как у своего сборщика.
                Err(mut e) => {
                    e.message = format!("[KyberSwap] {}", e.message);
                    return Err(e);
                }
                other => return other,
            }
        }
        let (q, env) = (self.p.quoting(), self.p.env());
        scope(
            Class::Trade,
            exec::trade::trade(&q, &env, &self.p.nonces, req),
        )
        .await
    }
}
