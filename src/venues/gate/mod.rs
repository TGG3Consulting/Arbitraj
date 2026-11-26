//! Файл: venues/gate/mod.rs
//! Что делает: адаптер биржи Gate (Gate.io, спот, API v4) — реализация контракта gateway/contract.rs (решение 135).
//! Всё общение с Gate — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «gate», feed.stale_after_ms; секреты gate.api_key, gate.api_secret.
//!
//! Только по официальной документации Gate API v4 (https://www.gate.com/docs/developers/apiv4/en/, Spot WebSocket v4
//! https://www.gate.com/docs/developers/apiv4/ws/en/) и официальным SDK (github.com/gate/gateapi-python, gate/gatews).
//! Ссылки — в шапке каждого файла и в README.md этой папки. Депозит зачисляется на спот («DONE: Funds Credited to Spot
//! Account», /wallet/deposits) — перевода между счетами нет.

pub mod account;
pub mod account_update;
pub mod all_balances;
pub mod all_balances_parse;
pub mod book;
pub mod book_version;
pub mod ctx;
pub mod delist;
pub mod deposit_addr;
pub mod deposit_hold;
pub mod deposit_info;
pub mod deposit_min;
pub mod deposit_plan;
pub mod deposits;
pub mod depth;
mod destinations;
pub mod errors;
pub mod fast_book;
pub mod fast_io;
pub mod fast_pick;
pub mod fast_rule;
pub mod feed;
pub mod fees;
pub mod gt_balance;
pub mod key_info;
pub mod market;
pub mod market_sell;
pub mod networks_parse;
pub mod num;
pub mod obu;
pub mod order_rules;
pub mod order_state;
pub mod pairs;
pub mod pairs_watch;
pub mod reply;
pub mod resolve;
pub mod rest;
pub mod subs;
pub mod symbols;
pub mod symbols_parse;
pub mod tap;
pub mod tap_snap;
pub mod trade;
pub mod transfers;
pub mod user_stream;
pub mod wallet;
pub mod wallet_cost;
pub mod withdraw_notice;
pub mod ws;
pub mod ws_conn;
pub mod ws_plan;
pub mod ws_snap;

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    Asset, Balance, CoinNetworkInfo, Cost, Quote, SendChannelInfo, Side, TradeRequest, TradeResult,
    Transfer, VenueId, WithdrawRequest,
};
use crate::gateway::contract::{ConnectionCheck, DexFamily, Venue, VenueKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::gateway::frozen::QuoteSnapshot;

use book::Books;
use ctx::Ctx;
use deposit_addr::DepositAddrs;
use fees::Fees;
use subs::{Parts, Subs};
use symbols::Symbols;
use wallet::Networks;
use withdraw_notice::Notices;

/// Адаптер Gate.
pub struct Gate {
    ctx: Ctx,
    destinations: destinations::Destinations,
    /// Текущая подписка: задачи соединений и обновление справочника пар (subs.rs).
    subs: Subs,
    symbols: Symbols,
    books: Books,
    fees: Fees,
    /// Справочник сетей и сведения монет (wallet.rs).
    nets: Networks,
    /// Адреса депозита в памяти (deposit_addr.rs).
    addrs: DepositAddrs,
    /// Событие «вывод в статусе FAIL» (withdraw_notice.rs): канал ядра — из подписки на рынок.
    notices: Notices,
}

impl Gate {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            destinations: destinations::Destinations::default(),
            subs: Subs::default(),
            symbols: Symbols::default(),
            books: Books::default(),
            fees: Fees::default(),
            nets: Networks::default(),
            addrs: DepositAddrs::default(),
            notices: Notices::default(),
        }
    }

    /// Ответ «не применимо» для функций контракта только у DEX. Принимает: что именно. Возвращает: NotApplicable.
    fn not_applicable(&self, what: &str) -> GatewayError {
        let m = format!("Gate — биржа: {what} к ней не применимо");
        self.ctx
            .tag(GatewayError::new(ErrorCode::NotApplicable, None, m))
    }
}

#[async_trait]
impl Venue for Gate {
    fn supports_settlement_order(&self) -> bool {
        true
    }
    fn supports_settlement_withdrawal(&self) -> bool {
        true
    }

    fn id(&self) -> VenueId {
        self.ctx.venue()
    }

    fn kind(&self) -> VenueKind {
        VenueKind::Cex
    }

    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        let mut result = account::check(&self.ctx).await;
        if result.ok {
            match self.destinations.reconcile(&self.ctx).await {
                Ok(0) => {}
                Ok(count) => result.details.push_str(&format!(
                    ". Gate подтвердила возможность вывода на {count} ранее отклонённых адресов."
                )),
                Err(e) => result.details.push_str(&format!(
                    ". Сверка ранее отклонённых адресов: {}",
                    e.message
                )),
            }
        }
        Ok(result)
    }

    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        wallet::coin_networks(&self.ctx, &self.nets, &self.symbols).await
    }

    async fn refresh_withdraw_tariff(
        &self,
        ticker: &str,
        network: &str,
    ) -> GatewayResult<Vec<CoinNetworkInfo>> {
        let before = self.nets.cached_tariff(ticker, network);
        let fresh = self.coin_networks_of(ticker).await?;
        if let Some(row) = fresh
            .iter()
            .find(|row| row.asset.ticker == ticker && row.network == network)
        {
            self.nets.refresh_tariff(before, row);
        }
        Ok(fresh)
    }

    /// Решение 113: у Gate есть запросы одной монеты (/spot/currencies/{c}, withdraw_status?currency=, currency_chains).
    async fn coin_networks_of(&self, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        wallet::coin_networks_of(&self.ctx, &self.nets, &self.symbols, ticker).await
    }

    /// Решение 387: сети монеты для приёма депозита по запросу ядра (минимум неизвестен) — вместе с адресом депозита,
    /// который даёт минимум (deposit_min.rs for_deposit). Сбой адреса справочник монеты не роняет.
    async fn deposit_networks_of(&self, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        deposit_min::for_deposit(&self.ctx, &self.nets, &self.addrs, &self.symbols, ticker).await
    }

    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        self.notices.set_out(out.clone());
        let parts = Parts {
            ctx: self.ctx.clone(),
            symbols: self.symbols.clone(),
            books: self.books.clone(),
            fees: self.fees.clone(),
            nets: self.nets.clone(),
            addrs: self.addrs.clone(),
        };
        self.subs.subscribe(&parts, assets, out).await
    }

    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        self.books
            .quote(&self.ctx, &self.fees, asset, side, qty)
            .await
    }

    /// Решение 217: снимок стакана монеты на момент решения (book.rs freeze — копия стакана из памяти, без запросов).
    /// Ошибки: как у котировки до прохода по уровням (стакана нет, устарел, комиссия, настройка).
    async fn quote_snapshot(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        let f = self.books.freeze(&self.ctx, &self.fees, asset, side)?;
        Ok(std::sync::Arc::new(f))
    }

    fn rest_book_prerequisite(&self, asset: &Asset) -> GatewayResult<()> {
        self.fees.get(&self.ctx, &self.ctx.symbol(&asset.ticker)?)?;
        Ok(())
    }

    /// Решение 238: снимок стакана по REST для ордера (стакан потока устарел) — book_version.rs snapshot (GET
    /// /spot/order_book), комиссия — тот же план, что у стакана потока (book.rs frozen).
    async fn quote_rest(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        let book = book_version::snapshot(&self.ctx, asset)
            .await
            .map_err(|mut e| {
                e.message = format!(
                    "REST-стакан {} не получен или не разобран: {}",
                    asset.ticker, e.message
                );
                e
            })?;
        let f = book::frozen(&self.ctx, &self.fees, asset, side, book).map_err(|mut e| {
            e.message = format!(
                "REST-стакан {} получен и разобран, ошибка подготовки котировки: {}",
                asset.ticker, e.message
            );
            e
        })?;
        Ok(std::sync::Arc::new(f))
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req, None).await
    }

    /// Решение 268: рыночная продажа — market_sell.rs (POST /spot/orders, type market).
    async fn trade_market_sell(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        market_sell::sell(&self.ctx, &self.symbols, req).await
    }

    /// Решение 134: те же правила, что у настоящего ордера (order_rules::ready). Пробного ордера у Gate нет.
    async fn check_order(&self, req: &TradeRequest) -> GatewayResult<()> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        order_rules::ready(&self.ctx, &self.symbols, req)
            .await
            .map(|_| ())
    }

    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        transfers::validate(&self.ctx, &req)?;
        let attempt =
            self.destinations
                .attempt(&self.ctx, &req.asset.ticker, &req.network, &req.address)?;
        let mut res = attempt
            .signed(transfers::submit(&self.ctx, req.clone()))
            .await;
        let denied = res.as_ref().err().is_some_and(|e| {
            e.code == ErrorCode::InvalidAddress
                && e.raw
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .is_some_and(|v| v["label"] == "ADDRESS_NOT_USED")
        });
        if (denied || res.is_ok())
            && let Err(e) = self.destinations.record(&self.ctx, &attempt, denied).await
        {
            let _ = self
                .ctx
                .journal
                .write(
                    crate::storage::journal::Level::Error,
                    "venue.withdraw_destination",
                    Some(ctx::ID),
                    e.message,
                    None,
                )
                .await;
        }
        if let Err(e) = &mut res {
            e.message = format!(
                "POST /withdrawals, {} {} в сети {}: {}",
                req.amount.normalize(),
                req.asset.ticker,
                req.network,
                e.message
            );
        }
        res
    }

    fn requires_withdraw_destination_check(&self) -> bool {
        true
    }

    async fn needs_withdraw_destination_address(
        &self,
        asset: &Asset,
        network: &str,
    ) -> GatewayResult<bool> {
        self.destinations
            .relevant(&self.ctx, &asset.ticker, network)
            .await
    }

    fn checks_cex_withdraw_destination(&self) -> bool {
        true
    }

    async fn check_withdraw_destination(
        &self,
        asset: &Asset,
        network: &str,
        address: &str,
    ) -> GatewayResult<()> {
        self.destinations
            .check(&self.ctx, &asset.ticker, network, address)
            .await
    }

    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        transfers::withdrawal_status(&self.ctx, &self.notices, venue_ref).await
    }

    /// Решение 231: итог ордера с неизвестным исходом — resolve.rs.
    async fn resolve_trade(
        &self,
        req: TradeRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<TradeResult>> {
        resolve::trade(&self.ctx, req, p).await
    }

    /// Решение 231: создан ли вывод с неизвестным исходом — resolve.rs.
    async fn resolve_withdraw(
        &self,
        req: WithdrawRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<Transfer>> {
        resolve::withdraw(&self.ctx, req, p).await
    }

    async fn deposit_address(&self, asset: &Asset, network: &str) -> GatewayResult<String> {
        deposit_addr::deposit_address(&self.ctx, &self.addrs, &self.nets, asset, network).await
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        deposits::deposit_status(&self.ctx, asset, tx_hash).await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx, None).await
    }

    /// Решение 166: удержания вывода — депозиты DEP_CREDITED (deposit_hold.rs).
    async fn withdraw_hold(&self, _quote: &str) -> GatewayResult<crate::core::types::WithdrawHold> {
        deposit_hold::withdraw_hold(&self.ctx).await
    }

    /// Решение 175: делистинги — delisting_time пар справочника (delist.rs).
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        delist::delistings(&self.ctx).await
    }

    /// Решение 144: все счета Gate (спот, маржа, фьючерсы, поставочные, опционы, Earn, итог) — all_balances.rs, только чтение.
    async fn account_balances(&self) -> GatewayResult<Vec<crate::core::types::AccountBalances>> {
        all_balances::account_balances(&self.ctx).await
    }

    /// Решение 113: баланс одной монеты — GET /spot/accounts?currency=.
    async fn balance(&self, asset: &str) -> GatewayResult<Option<Balance>> {
        Ok(account::balances(&self.ctx, Some(asset))
            .await?
            .into_iter()
            .find(|b| b.asset == asset))
    }

    async fn deposit_to_trading(
        &self,
        asset: &Asset,
        amount: Decimal,
    ) -> GatewayResult<Option<String>> {
        // Депозит Gate зачисляется на спот («Funds Credited to Spot Account») — переводить нечего.
        let _ = (asset, amount);
        Ok(None)
    }

    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        wallet_cost::withdraw_cost(&self.ctx, &self.nets, &self.symbols, asset, network, amount)
            .await
    }

    async fn recheck_coins(&self, _assets: Vec<Asset>) -> GatewayResult<()> {
        Err(self.not_applicable("проверка налогов токена и крючков пулов"))
    }

    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        Err(self.not_applicable("список видов пулов DEX"))
    }

    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        Err(self.not_applicable("список каналов отправки транзакций"))
    }

    async fn check_send_channel(&self, _id: &str) -> GatewayResult<ConnectionCheck> {
        Err(self.not_applicable("проверка канала отправки транзакций"))
    }

    // Решение 125: запись окна — tap.rs (кадры стакана — ws_conn.rs, лента и изменения — feed.rs, tap_snap.rs).
    async fn window_tap(
        &self,
        asset: &Asset,
        sink: crate::gateway::tap::TapSink,
    ) -> GatewayResult<crate::gateway::tap::TapInfo> {
        tap::start(&self.ctx, asset, sink)
    }

    async fn window_untap(&self, asset: &Asset) -> GatewayResult<()> {
        tap::stop(&self.ctx, asset);
        Ok(())
    }

    fn decode_tap(
        &self,
        stream: crate::gateway::tap::TapStream,
        data: &crate::gateway::tap::TapData,
    ) -> GatewayResult<crate::core::types::TapDecoded> {
        tap::decode(stream, data)
    }

    /// Решение 202: отписка одной пары есть (event unsubscribe), см. ws_conn.rs. Решение 213: scheduled = false —
    /// документация Gate НЕ говорит, что spot.order_book шлёт кадр без изменений книги (venue.book_interval — частота
    /// обновлений), а живые данные 07.11 это опровергли: у неликвидных пар (AIL, KILO, PELL, U, XCV) поле update ответа
    /// GET /spot/order_book старше 1,5–6,5 ч, кадров нет при подписке со status success. Тишина пары подтверждается
    /// по REST (confirm = true, book_version.rs), вслепую не переподписывается.
    fn feed_health_for(
        &self,
        pair: Option<(&str, &str)>,
    ) -> Option<crate::gateway::feed_health::FeedHealth> {
        Some(self.books.feed.health_for(false, true, true, pair))
    }

    /// Решение 213: версия стакана пары по REST (book_version.rs).
    async fn book_version(
        &self,
        ticker: &str,
    ) -> GatewayResult<crate::gateway::feed_health::BookVersion> {
        book_version::book_version(&self.ctx, ticker).await
    }

    /// Решение 213: тихая пара подтверждена — стакан свежий в котировке до until_ms (book.rs, confirm).
    fn book_confirmed(&self, ticker: &str, until_ms: i64) {
        self.books.confirm(ticker, until_ms);
    }

    /// Решение 367 (повторное ревью t163, Б1): снять подтверждение тихой пары (book.rs unconfirm).
    fn book_unconfirmed(&self, ticker: &str) {
        self.books.unconfirm(ticker);
    }

    async fn resubscribe_pair(&self, conn: &str, ticker: &str) -> GatewayResult<()> {
        use crate::gateway::feed_health::Cmd;
        self.books
            .feed
            .command(self.id(), conn, Cmd::Resub(ticker.to_string()))
    }

    async fn reconnect_conn(&self, conn: &str) -> GatewayResult<()> {
        use crate::gateway::feed_health::Cmd;
        self.books.feed.command(self.id(), conn, Cmd::Reconnect)
    }
}
