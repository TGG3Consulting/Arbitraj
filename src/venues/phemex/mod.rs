//! Файл: venues/phemex/mod.rs
//! Что делает: адаптер биржи Phemex (только спот) — реализация контракта gateway/contract.rs (решение 168).
//! Всё общение с Phemex — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «phemex», feed.stale_after_ms; секреты phemex.api_key, phemex.api_secret.
//!
//! Только по официальной документации Phemex (https://phemex-docs.github.io/, исходники — github.com/phemex-docs/
//! phemex-docs.github.io) и официальному примеру клиента github.com/phemex/phemex-python-api. Особенность Phemex —
//! масштабированные целые (…Ep, …Ev, …Er) со своим масштабом у каждой монеты: перевод только в num.rs, наружу — Decimal.
//! Справочник сетей — по одной монете за запрос (фоновый круг networks_crawl.rs). Ссылки и что не описано — README.md.

pub mod account;
pub mod all_balances;
pub mod book;
pub mod book_print;
pub mod ctx;
pub mod delist;
pub mod deposit_addr;
pub mod deposits;
pub mod depth;
pub mod errors;
pub mod fees;
pub mod fund_move;
pub mod history;
pub mod local_book;
pub mod market;
pub mod market_sell;
pub mod networks_crawl;
pub mod networks_parse;
pub mod num;
pub mod order_rules;
pub mod order_state;
pub mod pairs;
pub mod pairs_watch;
pub mod reply;
pub mod rest;
pub mod sign;
pub mod subs;
pub mod symbols;
pub mod symbols_parse;
pub mod trade;
pub mod transfers;
pub mod user_stream;
pub mod user_stream_conn;
pub mod wallet;
pub mod wallet_update;
pub mod ws;
pub mod ws_conn;

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    AccountBalances, Asset, Balance, CoinNetworkInfo, Cost, Quote, SendChannelInfo, Side,
    TradeRequest, TradeResult, Transfer, VenueId, WithdrawRequest,
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

/// Адаптер Phemex.
pub struct Phemex {
    ctx: Ctx,
    subs: Subs,
    symbols: Symbols,
    books: Books,
    fees: Fees,
    nets: Networks,
    addrs: DepositAddrs,
}

impl Phemex {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            subs: Subs::default(),
            symbols: Symbols::default(),
            books: Books::default(),
            fees: Fees::default(),
            nets: Networks::default(),
            addrs: DepositAddrs::default(),
        }
    }

    /// Ответ «не применимо» для функций контракта только для DEX.
    fn not_applicable(&self, what: &str) -> GatewayError {
        self.ctx.tag(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!("Phemex — биржа: {what} к ней не применимо"),
        ))
    }
}

#[async_trait]
impl Venue for Phemex {
    fn id(&self) -> VenueId {
        self.ctx.venue()
    }

    fn kind(&self) -> VenueKind {
        VenueKind::Cex
    }

    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        Ok(account::check(&self.ctx, &self.symbols).await)
    }

    /// Собранное фоновым кругом (по монете за запрос); первый вызов запускает круг — wallet.rs.
    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        wallet::coin_networks(&self.ctx, &self.symbols, &self.nets).await
    }

    /// Решение 113: сети одной монеты — свежими запросами chainCfg и asset/info.
    async fn coin_networks_of(&self, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
        wallet::coin_networks_of(&self.ctx, &self.nets, ticker).await
    }

    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        let parts = Parts {
            ctx: self.ctx.clone(),
            symbols: self.symbols.clone(),
            books: self.books.clone(),
            fees: self.fees.clone(),
        };
        self.subs.subscribe(&parts, assets, out).await
    }

    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        let symbol = self.symbols.get(&self.ctx, &asset.ticker)?.symbol;
        self.books
            .quote(&self.ctx, &self.fees, &symbol, asset, side, qty)
            .await
    }

    /// Решение 217: снимок стакана монеты на момент решения (book.rs freeze — копия стакана из памяти, без запросов).
    /// Ошибки: как у котировки до прохода по уровням (стакана нет, устарел, комиссия, настройка).
    async fn quote_snapshot(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        let symbol = self.symbols.get(&self.ctx, &asset.ticker)?.symbol;
        let f = self
            .books
            .freeze(&self.ctx, &self.fees, &symbol, asset, side)?;
        Ok(std::sync::Arc::new(f))
    }

    /// Решение 238: снимок стакана по REST для ордера (стакан потока устарел) — book_print.rs snapshot (GET
    /// /md/orderbook), комиссия — тот же план, что у стакана потока (book.rs frozen).
    async fn quote_rest(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        let rules = self.symbols.get(&self.ctx, &asset.ticker)?;
        let book = book_print::snapshot(&self.ctx, &rules, asset).await?;
        let f = book::frozen(&self.ctx, &self.fees, &rules.symbol, asset, side, book)?;
        Ok(std::sync::Arc::new(f))
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req).await
    }

    /// Решение 268: рыночная продажа — market_sell.rs (PUT /spot/orders/create, ordType Market).
    async fn trade_market_sell(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        market_sell::sell(&self.ctx, &self.symbols, req).await
    }

    /// Решение 134: те же правила, что у настоящего ордера (order_rules::ready); пробного ордера у Phemex нет.
    async fn check_order(&self, req: &TradeRequest) -> GatewayResult<()> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        order_rules::ready(&self.ctx, &self.symbols, req)
            .await
            .map(|_| ())
    }

    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        transfers::withdraw(&self.ctx, req).await
    }

    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        transfers::withdrawal_status(&self.ctx, &self.symbols, venue_ref).await
    }

    async fn deposit_address(&self, asset: &Asset, network: &str) -> GatewayResult<String> {
        self.addrs.get(&self.ctx, asset, network).await
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        deposits::deposit_status(&self.ctx, &self.symbols, asset, tx_hash).await
    }

    /// Решение 144: все счета Phemex для панели «Балансы» — all_balances.rs (только чтение).
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        all_balances::read(&self.ctx, &self.symbols).await
    }

    /// Решение 226: делистинги — timeline[3] («delist time») спот-пар GET /public/products-plus (delist.rs).
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        delist::delistings(&self.ctx).await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx, &self.symbols, None).await
    }

    /// Решение 113: баланс одной монеты — GET /spot/wallets?currency=.
    async fn balance(&self, asset: &str) -> GatewayResult<Option<Balance>> {
        Ok(account::balances(&self.ctx, &self.symbols, Some(asset))
            .await?
            .into_iter()
            .find(|b| b.asset == asset))
    }

    /// Решение 102: депозит на CONTRACT — перевод на SPOT (venue.deposit_to_spot), fund_move.rs.
    async fn deposit_to_trading(
        &self,
        asset: &Asset,
        amount: Decimal,
    ) -> GatewayResult<Option<String>> {
        fund_move::to_spot(&self.ctx, &self.symbols, asset, amount).await
    }

    /// Комиссия вывода — для этой суммы (asset/info с amount), wallet.rs.
    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        wallet::withdraw_cost(&self.ctx, asset, network, amount).await
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

    /// Решение 202 (для всех бирж, Тигран 08.11): свежесть соединений и пар стаканов (ws_conn.rs, gateway/feed_reg.rs).
    /// - scheduled = false: изменения — при изменении стакана, контрольный снимок — «published with 60-second interval
    ///   for client self-verification» (OrderBook message); слов «и без изменений» нет (как «every 1 minute» у CoinEx),
    ///   при confirm = true поле не влияет. Тишина пары дольше 60 с с запасом — признак потери (venue.book_stale_ms ×
    ///   venue.pair_dead_mult больше 60 000, docs/FIRST_RUN.md);
    /// - per_pair = true, как у CoinEx: отписки одной пары нет («orderbook.unsubscribe … unsubscribes all orderbook related
    ///   subscriptions»), поэтому «переподписать пару» = orderbook.unsubscribe + orderbook.subscribe всех пар соединения
    ///   (ws_conn.rs); соседи получают новый снимок и прогрев;
    /// - confirm = true (решение 213, по содержимому): связь sequence ответа GET /md/orderbook с sequence кадра в
    ///   документации не описана — сравниваются первые venue.depth_levels уровней ответа и стакана в памяти
    ///   (book_print.rs, core/feed_print.rs).
    fn feed_health(&self) -> Option<crate::gateway::feed_health::FeedHealth> {
        Some(self.books.feed.health(false, true, true))
    }

    /// Решение 213: отпечатки стакана пары для подтверждения тихой пары по содержимому (book_print.rs). Символ и масштабы
    /// — из справочника пар (symbols.rs). Ошибки: пары нет в справочнике, как у book_print::book_version.
    async fn book_version(
        &self,
        ticker: &str,
    ) -> GatewayResult<crate::gateway::feed_health::BookVersion> {
        let rules = self.symbols.get(&self.ctx, ticker)?;
        book_print::book_version(&self.ctx, &self.books, &rules, ticker).await
    }

    /// Решение 213: тихая пара подтверждена — стакан свежий в котировке до until_ms (book.rs, confirm; действует, если
    /// задана venue.book_stale_ms).
    fn book_confirmed(&self, ticker: &str, until_ms: i64) {
        self.books.confirm(ticker, until_ms);
    }

    /// Решение 202: переподписать пару — у Phemex отписка только всех пар соединения: orderbook.unsubscribe +
    /// orderbook.subscribe каждой пары соединения (ws_conn.rs).
    async fn resubscribe_pair(&self, conn: &str, ticker: &str) -> GatewayResult<()> {
        use crate::gateway::feed_health::Cmd;
        self.books
            .feed
            .command(self.id(), conn, Cmd::Resub(ticker.to_string()))
    }

    /// Решение 202: переподключить одно соединение с теми же парами (ws.rs подключается заново).
    async fn reconnect_conn(&self, conn: &str) -> GatewayResult<()> {
        use crate::gateway::feed_health::Cmd;
        self.books.feed.command(self.id(), conn, Cmd::Reconnect)
    }
}
