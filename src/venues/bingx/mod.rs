//! Файл: venues/bingx/mod.rs
//! Что делает: адаптер биржи BingX (спот) — реализация контракта gateway/contract.rs.
//! Всё общение с BingX — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «bingx», feed.stale_after_ms; секреты bingx.api_key, bingx.api_secret.
//!
//! Только по официальной документации BingX (CLAUDE.md, решение 23). Ссылки — в шапке каждого файла
//! и в README.md этой папки.

pub mod account;
pub mod account_update;
pub mod all_balances;
pub mod all_balances_parse;
pub mod book;
pub mod ctx;
pub mod delist;
pub mod deposit_addr;
pub mod depth;
pub mod errors;
pub mod feed;
pub mod fees;
pub mod fees_pause;
pub mod fund_move;
pub mod market;
pub mod num;
pub mod order_rules;
pub mod order_state;
pub mod pairs;
pub mod pairs_watch;
pub mod rest;
pub mod subs;
pub mod symbols;
pub mod tap;
pub mod tap_diff;
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

/// Адаптер BingX.
pub struct Bingx {
    ctx: Ctx,
    /// Текущая подписка: задачи соединений и обновление справочника пар (subs.rs).
    subs: Subs,
    symbols: Symbols,
    books: Books,
    fees: Fees,
    /// Адреса депозита в памяти (deposit_addr.rs, решение 118 Я-5).
    addrs: DepositAddrs,
}

impl Bingx {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            subs: Subs::default(),
            symbols: Symbols::default(),
            books: Books::default(),
            fees: Fees::default(),
            addrs: DepositAddrs::default(),
        }
    }

    /// Справочник пар адаптера (общий: клон делит те же данные). Тесты кладут в него записанный ответ common/symbols
    /// (Symbols::load) — проверка ордера check_order идёт по нему без сети. Ошибок нет.
    pub fn symbols(&self) -> Symbols {
        self.symbols.clone()
    }

    /// Ответ «не применимо» для функций контракта, которых у биржи нет (они только у DEX).
    /// Принимает: что именно не применимо. Возвращает: ошибку NotApplicable с площадкой BingX.
    fn not_applicable(&self, what: &str) -> GatewayError {
        self.ctx.tag(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!("BingX — биржа: {what} к ней не применимо"),
        ))
    }
}

#[async_trait]
impl Venue for Bingx {
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
        wallet::coin_networks(&self.ctx).await
    }

    /// Решение 175: делистинги — пары в статусе 29 «pre-delisted» справочника пар (delist.rs).
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        delist::delistings(&self.ctx, &self.symbols).await
    }

    /// Одна монета — config/getall?coin= (решение 113, wallet.rs).
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

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req).await
    }

    /// Решение 134: те же правила, что у настоящего ордера (order_rules::ready). Пробного ордера у спота BingX нет.
    async fn check_order(&self, req: &TradeRequest) -> GatewayResult<()> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        order_rules::ready(&self.ctx, &self.symbols, req).map(|_| ())
    }

    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        transfers::withdraw(&self.ctx, req).await
    }

    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        transfers::withdrawal_status(&self.ctx, venue_ref).await
    }

    async fn deposit_address(&self, asset: &Asset, network: &str) -> GatewayResult<String> {
        deposit_addr::deposit_address(&self.ctx, &self.addrs, asset, network).await
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        transfers::deposit_status(&self.ctx, asset, tx_hash).await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx).await
    }

    /// Решение 144: все счета BingX (спот, фонд, фьючерсы USDT-M / COIN-M / стандартные, обзор) — all_balances.rs.
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        all_balances::read(&self.ctx).await
    }

    /// Решение 174: роль счёта панели «Балансы» для кнопки «Продать» — названия те же, что в all_balances.rs: «Спот» и «Фонд (Funding)»; deposit_to_trading — fund → spot (fund_move.rs).
    fn account_role(&self, account: &str) -> AccountRole {
        match account {
            "Спот" => AccountRole::Trading,
            "Фонд (Funding)" => AccountRole::ToTrading,
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
        amount: rust_decimal::Decimal,
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

    // Решение 125: запись окна — tap.rs (кадры стакана — ws_conn.rs, лента сделок и изменения стакана — feed.rs).
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
}
