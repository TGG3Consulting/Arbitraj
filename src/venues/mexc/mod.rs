//! Файл: venues/mexc/mod.rs
//! Что делает: адаптер биржи MEXC (спот, API v3) — реализация контракта gateway/contract.rs (решение 107).
//! Всё общение с MEXC — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «mexc», feed.stale_after_ms; секреты mexc.api_key, mexc.api_secret.
//!
//! Только по официальной документации MEXC (https://www.mexc.com/api-docs/spot-v3/introduction). Ссылки — в шапке
//! каждого файла и в README.md этой папки. Фондового счёта у MEXC в документации нет: депозит сразу на споте.

pub mod account;
pub mod account_update;
pub mod all_balances;
pub mod book;
pub mod book_print;
pub mod book_rest;
pub mod ctx;
pub mod delist;
pub mod deposit_addr;
pub mod deposit_hold;
pub mod deposits;
pub mod depth;
pub mod errors;
pub mod errors_fail;
pub mod feed;
pub mod fees;
pub mod fills;
pub mod futures_rest;
pub mod ip_weight;
pub mod market;
pub mod market_sell;
pub mod networks_parse;
pub mod num;
pub mod order_rules;
pub mod order_state;
pub mod pairs;
pub mod pairs_watch;
pub mod proto;
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
pub mod user_key;
pub mod user_stream;
pub mod user_stream_conn;
pub mod wallet;
pub mod ws;
pub mod ws_conn;

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

/// Адаптер MEXC.
pub struct Mexc {
    ctx: Ctx,
    /// Текущая подписка: задачи соединений и обновление справочника пар (subs.rs).
    subs: Subs,
    symbols: Symbols,
    books: Books,
    fees: Fees,
    /// Коды сетей (netWork) → отображаемые имена и кэш справочника (wallet.rs).
    nets: Networks,
    /// Адреса депозита в памяти (deposit_addr.rs, решение 118 Я-5).
    addrs: DepositAddrs,
}

impl Mexc {
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

    /// Ответ «не применимо» для функций контракта, которых у биржи нет (они только у DEX).
    /// Принимает: что именно не применимо. Возвращает: ошибку NotApplicable с площадкой MEXC.
    fn not_applicable(&self, what: &str) -> GatewayError {
        self.ctx.tag(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!("MEXC — биржа: {what} к ней не применимо"),
        ))
    }
}

#[async_trait]
impl Venue for Mexc {
    fn id(&self) -> VenueId {
        self.ctx.venue()
    }

    fn kind(&self) -> VenueKind {
        VenueKind::Cex
    }

    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        Ok(account::check(&self.ctx).await)
    }

    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        wallet::coin_networks(&self.ctx, &self.nets, &self.symbols).await
    }

    async fn coin_networks_of(&self, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
        wallet::coin_networks_of(&self.ctx, &self.nets, &self.symbols, ticker).await
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

    /// Решение 238: снимок стакана по REST для ордера (стакан потока устарел) — book_print.rs snapshot (GET
    /// /api/v3/depth, вес 3), комиссия — тот же план, что у стакана потока (book.rs frozen).
    async fn quote_rest(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        let book = book_print::snapshot(&self.ctx, asset).await?;
        let f = book::frozen(&self.ctx, &self.fees, asset, side, book)?;
        Ok(std::sync::Arc::new(f))
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req, None).await
    }

    /// Решение 268: рыночная продажа — market_sell.rs (POST /api/v3/order, type MARKET).
    async fn trade_market_sell(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        market_sell::sell(&self.ctx, &self.symbols, req).await
    }

    /// Решение 134: те же правила, что у настоящего ордера (order_rules::ready), и пробный ордер, если включён.
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
        transfers::withdrawal_status(&self.ctx, venue_ref).await
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
        let (ctx, addrs, nets) = (&self.ctx, &self.addrs, &self.nets);
        deposit_addr::deposit_address(ctx, addrs, nets, &self.symbols, asset, network).await
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        deposits::deposit_status(&self.ctx, asset, tx_hash).await
    }

    /// Решение 226: делистинги — признаки риска делистинга пар GET /api/v3/exchangeInfo (st, status, tradeSideType;
    /// delist.rs), без времени — монета на MEXC не торгуется.
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        delist::delistings(&self.ctx).await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx).await
    }

    /// Решение 150: предварительно зачисленные депозиты (deposit/hisrec, статус 9) — их нельзя вывести.
    async fn withdraw_hold(&self, _quote: &str) -> GatewayResult<crate::core::types::WithdrawHold> {
        deposit_hold::withdraw_hold(&self.ctx).await
    }

    /// Решение 144: панель «Балансы» — счета «Спот» и «Фьючерсы» параллельно (all_balances.rs), только чтение.
    async fn account_balances(&self) -> GatewayResult<Vec<crate::core::types::AccountBalances>> {
        Ok(all_balances::all(&self.ctx).await)
    }

    async fn deposit_to_trading(
        &self,
        asset: &Asset,
        amount: rust_decimal::Decimal,
    ) -> GatewayResult<Option<String>> {
        // Фондового счёта у MEXC в документации нет: депозит зачисляется на спот, переводить нечего.
        let _ = (asset, amount);
        Ok(None)
    }

    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        _amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        wallet::withdraw_cost(&self.ctx, &self.nets, &self.symbols, asset, network).await
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

    // Решение 125: запись окна — tap.rs (кадры стакана — ws_conn.rs, лента сделок и изменения стакана — feed.rs, tap_snap.rs).
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

    /// Решение 202 (для всех бирж, Тигран 08.11): свежесть соединений и пар стаканов (ws_conn.rs, gateway/feed_reg.rs).
    /// - scheduled = false: частота частичного стакана spot@public.limit.depth.v3.api.pb в документации не указана
    ///   (Partial Book Depth Streams, https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/
    ///   partial-book-depth-streams), что кадр идёт и без изменений книги — не сказано;
    /// - per_pair = true: отписка одной подписки — {"method":"UNSUBSCRIPTION"} (Live Subscription/Unsubscription to Data
    ///   Streams), см. ws_conn.rs;
    /// - confirm = true (решение 213, по содержимому — Тигран 08.11): номер не сравнивается — поле version кадра
    ///   частичного стакана описано только как «Version number», связи с lastUpdateId ответа GET /api/v3/depth в
    ///   документации нет (с lastUpdateId связаны только fromVersion / toVersion канала изменений spot@public.aggre.depth
    ///   — «How to Properly Maintain a Local Copy of the Order Book»); сравниваются первые venue.depth_levels уровней
    ///   ответа REST и стакана в памяти (book_print.rs, core/feed_print.rs).
    fn feed_health(&self) -> Option<crate::gateway::feed_health::FeedHealth> {
        Some(self.books.feed.health(false, true, true))
    }

    /// Решение 213: отпечатки стакана пары для подтверждения тихой пары по содержимому (book_print.rs).
    async fn book_version(
        &self,
        ticker: &str,
    ) -> GatewayResult<crate::gateway::feed_health::BookVersion> {
        book_print::book_version(&self.ctx, &self.books, ticker).await
    }

    /// Решение 213: тихая пара подтверждена — стакан свежий в котировке до until_ms (book.rs, confirm).
    fn book_confirmed(&self, ticker: &str, until_ms: i64) {
        self.books.confirm(ticker, until_ms);
    }

    /// t79 (Тигран 10.11: согласовано): тихая пара «книга не менялась с переподписки» — уровни того же ответа REST
    /// стаканом пары (book_rest.rs put_rest: время кадра — получение ответа, свежесть — book.rs freeze).
    fn book_from_rest(
        &self,
        ticker: &str,
        levels: &crate::gateway::feed_health::BookPrint,
        sent_ms: i64,
        recv_ms: i64,
    ) -> GatewayResult<crate::core::types::OrderBook> {
        self.books
            .put_rest(self.id(), ticker, levels, sent_ms, recv_ms)
            .map_err(|why| {
                self.ctx.tag(crate::gateway::errors::GatewayError::new(
                    crate::gateway::errors::ErrorCode::StaleData,
                    None,
                    format!("MEXC {ticker}: {why}"),
                ))
            })
    }

    /// Решение 202: переподписать одну пару в её соединении (UNSUBSCRIPTION + SUBSCRIPTION, ws_conn.rs).
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
