//! Файл: venues/bitget/mod.rs
//! Что делает: адаптер биржи Bitget (спот, Classic API v2) — реализация контракта gateway/contract.rs (решение 162).
//! Всё общение с Bitget — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «bitget», feed.stale_after_ms; секреты bitget.api_key, bitget.api_secret,
//! bitget.api_passphrase.
//!
//! Только по официальной документации Bitget Classic (https://www.bitget.com/docs/classic/catalog) и официальному SDK
//! github.com/BitgetLimited/v3-bitget-api-sdk. Classic v2 — для классического счёта (у UTA нет справочника монет с
//! сетями); «Проверить подключение» показывает режим счёта, UTA — отказ. Счета: спот (торговля) и фонд (p2p/funding) —
//! переводы между ними по настройкам (fund_move.rs). Ссылки и что не описано — README.md этой папки.

pub mod account;
pub mod all_balances;
pub mod book;
pub mod book_decided;
pub mod book_print;
pub mod ctx;
pub mod delist;
pub mod delist_ann;
pub mod deposit_addr;
pub mod deposits;
pub mod depth;
pub mod errors;
pub mod fees;
pub mod fund_move;
pub mod market;
pub mod market_sell;
pub mod networks_parse;
pub mod num;
pub mod order_fee;
pub mod order_rules;
pub mod order_state;
pub mod pairs;
pub mod pairs_watch;
pub mod records;
pub mod reply;
pub mod resolve;
pub mod rest;
pub mod rest_book;
pub mod sign;
pub mod subs;
pub mod symbols;
pub mod symbols_parse;
pub mod trade;
pub mod transfers;
pub mod wallet;
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

/// Адаптер Bitget.
pub struct Bitget {
    ctx: Ctx,
    subs: Subs,
    symbols: Symbols,
    books: Books,
    fees: Fees,
    nets: Networks,
    addrs: DepositAddrs,
}

impl Bitget {
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
            format!("Bitget — биржа: {what} к ней не применимо"),
        ))
    }
}

#[async_trait]
impl Venue for Bitget {
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

    /// Решение 113: у Bitget есть запрос одной монеты (GET /api/v2/spot/public/coins?coin=).
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
        // Решение 201: у котировки исполнения справочник догружается, если пары ещё нет (symbols.rs get_exec).
        let symbol = self
            .symbols
            .get_exec(&self.ctx, &asset.ticker)
            .await?
            .symbol;
        // Решение 201: котировке исполнения при тихом или ещё не пришедшем стакане потока — снимок REST (rest_book.rs).
        if crate::gateway::patience::is_exec()
            && self.books.exec_needs_rest(&self.ctx, &asset.ticker)
        {
            let book = rest_book::snapshot(&self.ctx, &symbol, asset).await?;
            // Снимок — и в память адаптера, и ядру (худшая цена ордера core/worst.rs берёт стакан ядра). Ядро
            // принимает событие асинхронно: если ордер успеет раньше — худшая цена по прежнему стакану ядра.
            if self.books.put_rest(book.clone())
                && let Some(tx) = self.subs.sender()
            {
                let _ = tx.try_send(MarketEvent::Book(book.clone()));
            }
            return book::priced(&self.ctx, &self.fees, &symbol, asset, side, qty, book);
        }
        self.books
            .quote(&self.ctx, &self.fees, &symbol, asset, side, qty)
            .await
    }

    /// Решение 217: снимок стакана монеты на момент решения (book.rs freeze_decided — копия того стакана, по которому
    /// котировали: поток или снимок REST котировки исполнения, решение 201; без запросов в пути сделки).
    /// Ошибки: как у котировки до прохода по уровням (стакана нет, устарел, комиссия, настройка).
    /// Решение 238: на пути ордера (gateway/patience.rs is_exec — core/exec_book.rs) — только стакан потока (book.rs
    /// freeze, мера venue.book_stale_ms) со справочником исполнения (symbols.rs get_exec, решение 201); устарел — ядро
    /// берёт снимок REST (quote_rest), а не снимок прошлой котировки по сроку feed.stale_after_ms (одна мера свежести).
    async fn quote_snapshot(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        if crate::gateway::patience::is_exec() {
            let symbol = self
                .symbols
                .get_exec(&self.ctx, &asset.ticker)
                .await?
                .symbol;
            let f = self
                .books
                .freeze(&self.ctx, &self.fees, &symbol, asset, side)?;
            return Ok(std::sync::Arc::new(f));
        }
        let symbol = self.symbols.get(&self.ctx, &asset.ticker)?.symbol;
        let f = self
            .books
            .freeze_decided(&self.ctx, &self.fees, &symbol, asset, side)?;
        Ok(std::sync::Arc::new(f))
    }

    /// Решение 238: снимок стакана по REST для ордера (стакан потока устарел) — rest_book.rs snapshot (GET
    /// /api/v2/spot/market/orderbook, «20 times/1s (IP)»), как у котировки исполнения решения 201: снимок — и в память
    /// адаптера (put_rest), и ядру; комиссия — тот же план (book.rs frozen).
    async fn quote_rest(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        let symbol = self
            .symbols
            .get_exec(&self.ctx, &asset.ticker)
            .await?
            .symbol;
        let book = rest_book::snapshot(&self.ctx, &symbol, asset).await?;
        if self.books.put_rest(book.clone())
            && let Some(tx) = self.subs.sender()
        {
            let _ = tx.try_send(MarketEvent::Book(book.clone()));
        }
        let f = book::frozen(&self.ctx, &self.fees, &symbol, asset, side, book)?;
        Ok(std::sync::Arc::new(f))
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req).await
    }

    /// Решение 268: рыночная продажа — market_sell.rs (POST /api/v2/spot/trade/place-order, orderType market).
    async fn trade_market_sell(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        market_sell::sell(&self.ctx, &self.symbols, req).await
    }

    /// Решение 134: те же правила, что у настоящего ордера (order_rules::ready); пробного ордера у Bitget нет.
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
        self.addrs.get(&self.ctx, asset, network).await
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        deposits::deposit_status(&self.ctx, asset, tx_hash).await
    }

    /// Решение 144: все счета Bitget для панели «Балансы» — all_balances.rs (только чтение).
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        all_balances::read(&self.ctx).await
    }

    /// Решение 226: делистинги — offTime пар GET /api/v2/spot/public/symbols и анонсы делистинга (delist.rs).
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        delist::delistings(&self.ctx).await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx, None).await
    }

    /// Решение 113: баланс одной монеты — GET /api/v2/spot/account/assets?coin=.
    async fn balance(&self, asset: &str) -> GatewayResult<Option<Balance>> {
        Ok(account::balances(&self.ctx, Some(asset))
            .await?
            .into_iter()
            .find(|b| b.asset.eq_ignore_ascii_case(asset)))
    }

    /// Решение 102: депозит на фонд — перевод на спот (venue.deposit_to_spot), fund_move.rs.
    /// Решение 174: роль счёта панели «Балансы» для кнопки «Продать» — названия те же, что в all_balances.rs: «Спот» и «Фонд (Funding/P2P)»; deposit_to_trading — p2p → spot (fund_move.rs).
    fn account_role(&self, account: &str) -> AccountRole {
        match account {
            "Спот" => AccountRole::Trading,
            "Фонд (Funding/P2P)" => AccountRole::ToTrading,
            _ => AccountRole::Other,
        }
    }

    /// Решение 174: перевод со счёта ToTrading на торговый без условий deposit_to_trading (fund_move.rs).
    async fn move_to_trading(&self, asset: &Asset, amount: Decimal) -> GatewayResult<String> {
        fund_move::move_funds(
            &self.ctx,
            &asset.ticker,
            amount,
            fund_move::FUNDING,
            fund_move::SPOT,
        )
        .await
    }

    async fn deposit_to_trading(
        &self,
        asset: &Asset,
        amount: Decimal,
    ) -> GatewayResult<Option<String>> {
        fund_move::to_spot(&self.ctx, asset, amount).await
    }

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
    /// - scheduled = false: Depth Channel (https://www.bitget.com/docs/classic/websocket/spot/public/Depth-Channel) —
    ///   «Default data push frequency for books, books5, books15 is 200ms», что кадр идёт и без изменений книги — не
    ///   сказано; живой случай RWA 06.11 (решение 201) — пара молчала 45–55 с при живом соединении;
    /// - per_pair = true: отписка — op «unsubscribe» (WebSocket intro, https://www.bitget.com/docs/classic/websocket/
    ///   intro), см. ws_conn.rs;
    /// - confirm = true (решение 213, по содержимому — Тигран 08.11): номер не сравнивается — seq описан только у кадра
    ///   WebSocket («It increases when the order book is updated»), связи seq с ответом GET /api/v2/spot/market/orderbook
    ///   в документации нет; в выписке ответа (rest_book.rs, docs/notes/bitget-analysis-2026-11-05.md §5) — asks, bids,
    ///   ts, номера книги нет; сравниваются первые venue.depth_levels уровней ответа REST и стакана в памяти
    ///   (book_print.rs, core/feed_print.rs).
    fn feed_health(&self) -> Option<crate::gateway::feed_health::FeedHealth> {
        Some(self.books.feed.health(false, true, true))
    }

    /// Решение 213: отпечатки стакана пары для подтверждения тихой пары по содержимому (book_print.rs). Символ — из
    /// справочника пар (symbols.rs). Ошибки: пары нет в справочнике (BadResponse), как у book_print::book_version.
    async fn book_version(
        &self,
        ticker: &str,
    ) -> GatewayResult<crate::gateway::feed_health::BookVersion> {
        let symbol = self.symbols.get(&self.ctx, ticker)?.symbol;
        book_print::book_version(&self.ctx, &self.books, &symbol, ticker).await
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
