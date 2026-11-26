//! Файл: venues/htx/mod.rs
//! Что делает: адаптер биржи HTX (бывший Huobi; спот, API v1/v2) — реализация контракта gateway/contract.rs (решение
//! 171). Всё общение с HTX — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «htx», feed.stale_after_ms; секреты htx.api_key, htx.api_secret.
//!
//! Только по официальной документации HTX Spot API (https://huobiapi.github.io/docs/spot/v1/en/) и официальному SDK
//! github.com/HuobiRDCenter/huobi_Golang. Только спот: фьючерсов, маржи и переводов между счетами в адаптере нет
//! (депозит зачисляется на спот — отдельного фонда у спот-API нет). Адреса контракта BEP20 в документации нет: без
//! venue.contract_field монеты HTX не сопоставляются (сделок нет). Ссылки и что не описано — README.md этой папки.

pub mod account;
pub mod account_update;
pub mod accounts;
pub mod all_balances;
pub mod book;
pub mod book_fast;
pub mod book_mode;
pub mod book_print;
pub mod book_rule;
pub mod ctx;
pub mod delist;
pub mod deposit_addr;
pub mod deposits;
pub mod depth;
pub mod errors;
pub mod fast_book;
pub mod fast_conn;
pub mod fast_log;
pub mod fast_pair;
pub mod fast_reqs;
pub mod fast_resync;
pub mod fast_ws;
pub mod fees;
pub mod market;
pub mod market_sell;
pub mod mbp;
pub mod networks_parse;
pub mod num;
pub mod order_fee;
pub mod order_rules;
pub mod order_state;
pub mod pairs;
pub mod pairs_watch;
pub mod records;
pub mod reply;
pub mod rest;
pub mod sign;
pub mod snap_out;
pub mod subs;
pub mod symbols;
pub mod symbols_parse;
pub mod trade;
pub mod transfers;
pub mod user_stream;
pub mod user_stream_conn;
pub mod wallet;
mod withdraw_address;
mod withdraw_record;
pub mod ws;
pub mod ws_conn;

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    AccountBalances, Asset, Balance, CoinNetworkInfo, Cost, Quote, SendChannelInfo, Side,
    TradeRequest, TradeResult, Transfer, VenueId, WithdrawHold, WithdrawRequest,
};
use crate::gateway::contract::{ConnectionCheck, DexFamily, Venue, VenueKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::gateway::frozen::QuoteSnapshot;

use accounts::SpotId;
use book::Books;
use ctx::Ctx;
use deposit_addr::DepositAddrs;
use fees::Fees;
use subs::{Parts, Subs};
use symbols::Symbols;
use wallet::Networks;

/// Адаптер HTX.
pub struct Htx {
    ctx: Ctx,
    subs: Subs,
    symbols: Symbols,
    books: Books,
    fees: Fees,
    nets: Networks,
    addrs: DepositAddrs,
    spot: SpotId,
}

impl Htx {
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
            spot: SpotId::default(),
        }
    }

    /// Ответ «не применимо» для функций контракта только для DEX.
    fn not_applicable(&self, what: &str) -> GatewayError {
        self.ctx.tag(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!("HTX — биржа: {what} к ней не применимо"),
        ))
    }
}

#[async_trait]
impl Venue for Htx {
    fn supports_settlement_credit(&self) -> bool {
        true
    }
    fn supports_settlement_withdrawal(&self) -> bool {
        true
    }
    async fn resolve_withdraw(
        &self,
        req: WithdrawRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<Transfer>> {
        transfers::recover_settlement(&self.ctx, req, p).await
    }

    /// GET сверяет окончательный ордер по ID из заявки. Открытый ордер остаётся на сверке.
    async fn inspect_settlement_order(
        &self,
        req: TradeRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<TradeResult>> {
        order_state::inspect(&self.ctx, req, p).await
    }
    fn supports_settlement_order(&self) -> bool {
        true
    }
    /// Подготовленный символ проверяется по контракту спотового ордера. https://huobiapi.github.io/docs/spot/v1/en/#get-the-order-detail-of-an-order-based-on-client-order-id
    fn matches_settlement_order(
        &self,
        p: &crate::gateway::pending::PendingRef,
        asset: &str,
        quote: &str,
    ) -> bool {
        p.symbol
            .as_deref()
            .is_some_and(|s| s == format!("{asset}{quote}").to_lowercase())
    }

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

    /// Решение 113: у HTX есть запрос одной монеты (GET /v2/reference/currencies?currency=).
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
            spot: self.spot.clone(),
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
    /// /market/depth), комиссия — тот же план, что у стакана потока (book.rs frozen).
    async fn quote_rest(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        let symbol = self.symbols.get(&self.ctx, &asset.ticker)?.symbol;
        let book = book_print::snapshot(&self.ctx, &symbol, asset).await?;
        let f = book::frozen(&self.ctx, &self.fees, &symbol, asset, side, book)?;
        Ok(std::sync::Arc::new(f))
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, &self.spot, req, None).await
    }

    /// Решение 268: рыночная продажа — market_sell.rs (POST /v1/order/orders/place, type sell-market).
    async fn trade_market_sell(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        market_sell::sell(&self.ctx, &self.symbols, &self.spot, req).await
    }

    /// Решение 134: те же правила, что у настоящего ордера (order_rules::ready); пробного ордера у HTX нет.
    async fn check_order(&self, req: &TradeRequest) -> GatewayResult<()> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        order_rules::ready(&self.ctx, &self.symbols, &self.spot, req)
            .await
            .map(|_| ())
    }

    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        let info = wallet::withdraw_network(&self.ctx, &req.asset.ticker, &req.network).await?;
        transfers::withdraw(&self.ctx, req, &info).await
    }

    fn use_net_withdrawal_budget(&self) -> bool {
        true
    }

    async fn withdraw_with_network(
        &self,
        req: WithdrawRequest,
        info: &CoinNetworkInfo,
    ) -> GatewayResult<Transfer> {
        transfers::withdraw(&self.ctx, req, info).await
    }

    fn requires_withdraw_destination_check(&self) -> bool {
        true
    }

    /// Та же адресная книга ключа и те же идентификаторы сетей, что перед POST вывода.
    /// GET /v2/account/withdraw/address: https://www.htx.com/en-us/opend/newApiPages/?id=7ec50654-7773-11ed-9966-0242ac110003
    async fn check_withdraw_destination(
        &self,
        asset: &Asset,
        network: &str,
        address: &str,
    ) -> GatewayResult<()> {
        let chain =
            wallet::address_check_chain(&self.ctx, &self.nets, &asset.ticker, network).await?;
        withdraw_address::check(&self.ctx, &asset.ticker, &chain, address).await
    }

    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        transfers::withdrawal_status(&self.ctx, venue_ref).await
    }

    async fn withdrawal_status_for(&self, created: &Transfer) -> GatewayResult<Transfer> {
        let mut found = transfers::withdrawal_status(&self.ctx, &created.venue_ref).await?;
        if created.venue_ref.splitn(3, '/').count() == 3
            && found.asset.ticker == created.asset.ticker
        {
            found.asset = created.asset.clone();
            found.network = created.network.clone();
        }
        Ok(found)
    }

    async fn deposit_address(&self, asset: &Asset, network: &str) -> GatewayResult<String> {
        self.addrs.get(&self.ctx, asset, network).await
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        deposits::deposit_status(&self.ctx, asset, tx_hash).await
    }

    /// Решение 144: все счета HTX для панели «Балансы» — all_balances.rs (только чтение).
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        all_balances::read(&self.ctx).await
    }

    /// Решение 226: делистинги — dl («delist») и tca («trade close at») пар GET /v1/settings/common/symbols (delist.rs).
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        delist::delistings(&self.ctx).await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx, &self.spot).await
    }

    /// Решение 150: депозиты в состоянии confirmed — торговать можно, вывести нельзя до safe (deposits.rs).
    async fn withdraw_hold(&self, _quote: &str) -> GatewayResult<WithdrawHold> {
        deposits::withdraw_hold(&self.ctx).await
    }

    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        _amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        wallet::withdraw_cost(&self.ctx, asset, network).await
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
    /// - scheduled = false: mbp.refresh — «The update interval is around 100ms»; шлёт ли тема снимок без изменений, не
    ///   сказано; при confirm = true поле не влияет;
    /// - per_pair = true: «Websocket Market Data → Unsubscribe» — {"unsub": тема, "id"} одной пары (ws_conn.rs);
    /// - confirm = true (решение 213, по содержимому): seqNum снимка и version ответа GET /market/depth («Internal data»)
    ///   документация не связывает — сравниваются первые venue.depth_levels уровней ответа и стакана в памяти
    ///   (book_print.rs, core/feed_print.rs).
    fn feed_health_for(
        &self,
        pair: Option<(&str, &str)>,
    ) -> Option<crate::gateway::feed_health::FeedHealth> {
        Some(self.books.feed.health_for(false, true, true, pair))
    }

    /// Решение 213: отпечатки стакана пары для подтверждения тихой пары по содержимому (book_print.rs). Символ — из
    /// справочника пар (symbols.rs). Ошибки: пары нет в справочнике, как у book_print::book_version.
    async fn book_version(
        &self,
        ticker: &str,
    ) -> GatewayResult<crate::gateway::feed_health::BookVersion> {
        let symbol = self.symbols.get(&self.ctx, ticker)?.symbol;
        book_print::book_version(&self.ctx, &self.books, &symbol, ticker).await
    }

    /// Решение 213: тихая пара подтверждена — стакан свежий в котировке до until_ms (book.rs, confirm; действует, если
    /// задана venue.book_stale_ms).
    fn book_confirmed(&self, ticker: &str, until_ms: i64) {
        self.books.confirm(ticker, until_ms);
    }

    /// Решение 367 (повторное ревью t163, Б1): снять подтверждение тихой пары (book.rs unconfirm).
    fn book_unconfirmed(&self, ticker: &str) {
        self.books.unconfirm(ticker);
    }

    /// Решение 202: переподписать одну пару в её соединении (unsub + sub темы mbp.refresh, ws_conn.rs).
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
