//! Файл: venues/xt/mod.rs
//! Что делает: адаптер биржи XT.com (спот, API v4) — реализация контракта gateway/contract.rs (решение 167).
//! Всё общение с XT — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «xt», feed.stale_after_ms; секреты xt.api_key, xt.api_secret.
//!
//! Только по официальной документации XT (https://doc.xt.com/, исходник — github.com/xt-com/xt4-api). Ссылки — в
//! шапке каждого файла и в README.md папки. Разбор перед подключением — docs/notes/xt-analysis-2026-11-05.md.

pub mod account;
pub mod all_balances;
pub mod book;
pub mod book_version;
pub mod ctx;
pub mod deposit_addr;
pub mod deposits;
pub mod depth;
pub mod errors;
pub mod futures;
pub mod key_check;
pub mod local_book;
pub mod market;
pub mod networks_parse;
pub mod num;
pub mod order_rules;
pub mod order_state;
pub mod pairs;
pub mod pairs_watch;
pub mod range;
pub mod reply;
pub mod resolve;
pub mod rest;
pub mod sign;
pub mod snapshots;
pub mod subs;
pub mod symbols;
pub mod symbols_parse;
pub mod trade;
pub mod transfers;
pub mod wallet;
pub mod ws;
pub mod ws_book;
pub mod ws_conn;
pub mod ws_diff;

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
use subs::{Parts, Subs};
use symbols::Symbols;
use wallet::Networks;

/// Адаптер XT.com.
pub struct Xt {
    ctx: Ctx,
    /// Текущая подписка: задачи соединений и обновление справочника пар (subs.rs).
    subs: Subs,
    symbols: Symbols,
    books: Books,
    /// Справочник монет и сетей (wallet.rs).
    nets: Networks,
    /// Адреса депозита в памяти (deposit_addr.rs).
    addrs: DepositAddrs,
}

impl Xt {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            subs: Subs::default(),
            symbols: Symbols::default(),
            books: Books::default(),
            nets: Networks::default(),
            addrs: DepositAddrs::default(),
        }
    }

    /// Ответ «не применимо» для функций только DEX. Ошибок нет.
    fn not_applicable(&self, what: &str) -> GatewayError {
        self.ctx.tag(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!("XT — биржа: {what} к ней не применимо"),
        ))
    }
}

#[async_trait]
impl Venue for Xt {
    fn id(&self) -> VenueId {
        self.ctx.venue()
    }

    fn kind(&self) -> VenueKind {
        VenueKind::Cex
    }

    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        Ok(key_check::check(&self.ctx).await)
    }

    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        wallet::coin_networks(&self.ctx, &self.nets).await
    }

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
        };
        self.subs.subscribe(&parts, assets, out).await
    }

    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        self.books
            .quote(&self.ctx, &self.symbols, asset, side, qty)
            .await
    }

    /// Решение 217: снимок стакана монеты на момент решения (book.rs freeze — копия стакана из памяти, без запросов).
    /// Ошибки: как у котировки до прохода по уровням (стакана нет, устарел, комиссия, настройка).
    async fn quote_snapshot(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        let f = self.books.freeze(&self.ctx, &self.symbols, asset, side)?;
        Ok(std::sync::Arc::new(f))
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req).await
    }

    /// Решение 134: те же правила, что у настоящего ордера (order_rules::ready); пробного ордера у XT нет.
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
        deposit_addr::deposit_address(&self.ctx, &self.addrs, asset, network).await
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        deposits::deposit_status(&self.ctx, asset, tx_hash).await
    }

    /// Решение 175 / 226: делистинги — «неизвестно»: в GET /v4/public/symbol поля nextState / nextStateTime в документации без описания (в примере null) и пусты у всех 1110 пар в живом ответе 08.11.2026; state DELISTED — пара уже снята (решение 226: не используется, вопрос Тиграну).
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        Ok(crate::core::types::Delistings::unknown(
            "в GET /v4/public/symbol поля nextState / nextStateTime в документации без описания (в примере null) и пусты у всех 1110 пар в живом ответе 08.11.2026; state DELISTED — пара уже снята (решение 226: не используется, вопрос Тиграну)",
        ))
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx).await
    }

    /// Решение 113: у XT есть баланс одной монеты (GET /v4/balance).
    async fn balance(&self, asset: &str) -> GatewayResult<Option<Balance>> {
        account::balance(&self.ctx, asset).await
    }

    /// Решение 144: «Спот», «Фьючерсы USDT-M», «Фьючерсы Coin-M», «Маржа» (all_balances.rs), только чтение.
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        Ok(all_balances::all(&self.ctx).await)
    }

    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        _amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        wallet::withdraw_cost(&self.ctx, &self.nets, asset, network).await
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
    /// - scheduled = false: «rate: 1000ms» (depth@, limitDepth.md) и «rate: 100ms» (depth_update@, increDepth.md) — что
    ///   кадр идёт и без изменений книги, документация не говорит (https://doc.xt.com/#websocket_publiclimitDepth);
    /// - per_pair = true: отписка одной темы — {"method":"unsubscribe"} (Request message format,
    ///   https://doc.xt.com/#websocket_publicrequestFormat), см. ws_conn.rs;
    /// - confirm = true (решение 213): depth_update — по номеру (lastUpdateId REST ↔ номер локального стакана,
    ///   «Orderbook manage»), depth — по содержимому (связи updateId снимка с REST нет) — book_version.rs.
    fn feed_health(&self) -> Option<crate::gateway::feed_health::FeedHealth> {
        Some(self.books.feed.health(false, true, true))
    }

    /// Решение 213: версия стакана пары по REST GET /v4/public/depth — номер или отпечатки (book_version.rs).
    async fn book_version(
        &self,
        ticker: &str,
    ) -> GatewayResult<crate::gateway::feed_health::BookVersion> {
        book_version::book_version(&self.ctx, &self.symbols, &self.books, ticker).await
    }

    /// Решение 213: тихая пара подтверждена — стакан свежий в котировке до until_ms (book.rs, confirm).
    fn book_confirmed(&self, ticker: &str, until_ms: i64) {
        self.books.confirm(ticker, until_ms);
    }

    /// Решение 202: переподписать одну пару в её соединении (unsubscribe + subscribe, ws_conn.rs).
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
