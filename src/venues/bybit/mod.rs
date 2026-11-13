//! Файл: venues/bybit/mod.rs
//! Что делает: адаптер биржи Bybit (API V5, только спот: category=spot) — реализация контракта gateway/contract.rs
//! (решение 136). Всё общение с Bybit — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «bybit», feed.stale_after_ms; секреты bybit.api_key, bybit.api_secret.
//!
//! Только по официальной документации Bybit V5 (https://bybit-exchange.github.io/docs/v5/intro; исходник —
//! github.com/bybit-exchange/docs). Ссылки — в шапке каждого файла и в README.md этой папки. Фьючерсов, маржи и
//! опционов нет: каждый запрос — category=spot, ордер — isLeverage=0, ключ с правами деривативов не проходит проверку.
//! Счёт: единый торговый (UNIFIED) — торговля и балансы; депозит — на UNIFIED по настройке кабинета (решение 136, Тигран
//! 04.11), запасной вариант — перевод FUND → UNIFIED (fund_move.rs, venue.deposit_to_spot).

pub mod account;
pub mod all_balances;
pub mod all_balances_parse;
pub mod book;
pub mod book_print;
pub mod ctx;
pub mod delist;
pub mod delist_ann;
pub mod deposit_addr;
pub mod deposits;
pub mod depth;
pub mod errors;
pub mod errors_fail;
pub mod feed;
pub mod fees;
pub mod fills;
pub mod fund_move;
pub mod http;
pub mod local_book;
pub mod market;
pub mod market_sell;
pub mod networks_parse;
pub mod num;
pub mod order_rules;
pub mod order_state;
pub mod pairs;
pub mod pairs_watch;
pub mod private_cache;
pub mod reply;
pub mod resolve;
pub mod rest;
pub mod subs;
pub mod symbols;
pub mod symbols_parse;
pub mod tap;
pub mod trade;
pub mod transfers;
pub mod user_stream;
pub mod user_stream_conn;
pub mod wallet;
pub mod withdrawable;
pub mod ws;
pub mod ws_conn;

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    AccountBalances, Asset, Balance, CoinNetworkInfo, Cost, Quote, SendChannelInfo, Side,
    TradeRequest, TradeResult, Transfer, VenueId, WithdrawRequest,
};
use crate::gateway::contract::{AccountRole, ConnectionCheck, DexFamily, Venue, VenueKind};
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

/// Адаптер Bybit.
pub struct Bybit {
    ctx: Ctx,
    subs: Subs,
    symbols: Symbols,
    books: Books,
    fees: Fees,
    nets: Networks,
    addrs: DepositAddrs,
}

impl Bybit {
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
    fn not_applicable(&self, what: &str) -> GatewayError {
        self.ctx.tag(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!("Bybit — биржа: {what} к ней не применимо"),
        ))
    }
}

#[async_trait]
impl Venue for Bybit {
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
        wallet::coin_networks(&self.ctx, &self.nets).await
    }

    /// Решение 113: у Bybit есть запрос одной монеты (coin/query-info?coin=).
    async fn coin_networks_of(&self, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
        wallet::coin_networks_of(&self.ctx, ticker).await
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

    /// Решение 238: снимок стакана по REST для ордера (стакан потока устарел) — book_print.rs snapshot
    /// (GET /v5/market/orderbook), комиссия — тот же план, что у стакана потока (book.rs frozen).
    async fn quote_rest(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        let book = book_print::snapshot(&self.ctx, asset).await?;
        let f = book::frozen(&self.ctx, &self.fees, asset, side, book)?;
        Ok(std::sync::Arc::new(f))
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req, None).await
    }

    /// Решение 268: рыночная продажа — market_sell.rs (POST /v5/order/create, orderType Market).
    async fn trade_market_sell(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        market_sell::sell(&self.ctx, &self.symbols, req).await
    }

    /// Решение 134: те же правила, что у настоящего ордера (order_rules::ready); пробного ордера у спота Bybit нет.
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

    /// Решение 231 (дополнение 08.11): вывод с неизвестным исходом — совпадение «ровно одна запись», resolve.rs.
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

    /// Решение 226: делистинги — метка stTag спот-пар GET /v5/market/instruments-info + анонсы delistings / Spot (delist.rs, delist_ann.rs).
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        delist::delistings(&self.ctx).await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx).await
    }

    /// Решение 113: у Bybit есть запрос баланса одной монеты (wallet-balance&coin=).
    async fn balance(&self, asset: &str) -> GatewayResult<Option<Balance>> {
        account::balance(&self.ctx, asset).await
    }

    /// Решение 166: удержание вывода — доступно минус «можно вывести» по счетам вывода, withdrawable.rs.
    async fn withdraw_hold(&self, quote: &str) -> GatewayResult<crate::core::types::WithdrawHold> {
        withdrawable::withdraw_hold(&self.ctx, quote).await
    }

    /// Решение 144: все счета Bybit (UNIFIED, FUND, прочие из Asset Overview) — all_balances.rs, только чтение.
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        all_balances::read(&self.ctx).await
    }

    /// Решение 102 для Bybit: запасной вариант — депозит на FUND → перевод на UNIFIED (venue.deposit_to_spot; по решению
    /// Тиграна 04.11 не задаётся: в кабинете депозит назначен на UNIFIED).
    /// Решение 174: роль счёта панели «Балансы» для кнопки «Продать» — названия те же, что в all_balances_parse.rs: единый торговый (UNIFIED) и «Фонд (Funding)» (FUND); deposit_to_trading — FUND → UNIFIED (fund_move.rs).
    fn account_role(&self, account: &str) -> AccountRole {
        match account {
            a if a == all_balances_parse::UNIFIED => AccountRole::Trading,
            a if a == all_balances_parse::FUND => AccountRole::ToTrading,
            _ => AccountRole::Other,
        }
    }

    /// Решение 174: перевод со счёта ToTrading на торговый без условий deposit_to_trading (fund_move.rs).
    async fn move_to_trading(&self, asset: &Asset, amount: Decimal) -> GatewayResult<String> {
        fund_move::transfer(&self.ctx, asset, amount).await
    }

    async fn deposit_to_trading(
        &self,
        asset: &Asset,
        amount: Decimal,
    ) -> GatewayResult<Option<String>> {
        fund_move::to_trading(&self.ctx, asset, amount).await
    }

    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        wallet::withdraw_cost(&self.ctx, &self.nets, asset, network, amount).await
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

    // Решение 125: запись окна — tap.rs (кадры стакана — ws_conn.rs, лента сделок и стакан окна — feed.rs).
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
    /// - scheduled = false: Orderbook (https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook) — «The
    ///   WebSocket will keep pushing delta messages every time the orderbook changes»; повтор снимка без изменений — только
    ///   у уровня 1 («Linear, inverse, spot level 1 data: if 3 seconds have elapsed without a change in the orderbook, a
    ///   snapshot message will be pushed again»); у остальных глубин тишина пары — не смерть;
    /// - per_pair = true: отписка одной темы — {"op":"unsubscribe"} (Connect, «Understanding WebSocket Filters:
    ///   Unsubscription»), см. ws_conn.rs;
    /// - confirm = true (решение 213): book_print.rs — по номеру u только при venue.depth_levels = 1000 (документация
    ///   связывает u REST с u потока 1000 уровней), иначе по содержимому (первые venue.depth_levels уровней ответа
    ///   GET /v5/market/orderbook и стакана в памяти, core/feed_print.rs).
    fn feed_health(&self) -> Option<crate::gateway::feed_health::FeedHealth> {
        Some(self.books.feed.health(false, true, true))
    }

    /// Решение 213: версия стакана пары для подтверждения тихой пары (book_print.rs).
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
