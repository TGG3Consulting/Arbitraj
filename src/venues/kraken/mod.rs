//! Файл: venues/kraken/mod.rs
//! Что делает: адаптер биржи Kraken (спот) — реализация контракта gateway/contract.rs (решение 172).
//! Всё общение с Kraken — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «kraken», feed.stale_after_ms; секреты kraken.api_key, kraken.api_secret.
//!
//! Только по официальной документации Kraken (https://docs.kraken.com/exchange/…, https://docs.kraken.com/api-reference/…,
//! OpenAPI https://docs.kraken.com/openapi/spot-rest.yaml). Спот REST /0 (пары, ордера, балансы, ставки), Funding (Beta)
//! /funding (методы с адресом контракта, адреса, выводы, депозиты), WebSocket v2 (стакан book с чек-суммой, balances).
//! Подписанные запросы одним ключом — по одному (nonce, ctx.rs). Счёт у Kraken один (спот) — переводов между счетами
//! нет. Ссылки и разбор — README.md этой папки и docs/notes/kraken-analysis-2026-11-05.md.

pub mod account;
pub mod all_balances;
pub mod balance_event;
pub mod book;
pub mod checksum;
pub mod ctx;
pub mod deposit_addr;
pub mod deposits;
pub mod depth;
pub mod errors;
pub mod fees;
pub mod local_book;
pub mod market;
pub mod methods;
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
pub mod withdraw_addr;
pub mod ws;
pub mod ws_conn;
pub mod ws_req;

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

use book::Books;
use ctx::Ctx;
use deposit_addr::DepositAddrs;
use fees::Fees;
use subs::{Parts, Subs};
use symbols::Symbols;
use wallet::Networks;

/// Адаптер Kraken.
pub struct Kraken {
    ctx: Ctx,
    subs: Subs,
    symbols: Symbols,
    books: Books,
    fees: Fees,
    nets: Networks,
    addrs: DepositAddrs,
}

impl Kraken {
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
            format!("Kraken — биржа: {what} к ней не применимо"),
        ))
    }
}

#[async_trait]
impl Venue for Kraken {
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

    /// Решение 113: у Kraken справочник одной монеты — те же методы Funding с asset[name] и Assets с asset=.
    async fn coin_networks_of(&self, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
        Ok(wallet::one(&self.ctx, ticker).await?.list)
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

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req).await
    }

    /// Решение 134: те же правила, что у настоящего ордера (order_rules::ready), и пробный ордер validate=true при
    /// venue.order_test_first = да.
    async fn check_order(&self, req: &TradeRequest) -> GatewayResult<()> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        let p = order_rules::ready(&self.ctx, &self.symbols, req).await?;
        order_rules::probe(&self.ctx, &p).await
    }

    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        transfers::withdraw(&self.ctx, req).await
    }

    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        transfers::withdrawal_status(&self.ctx, venue_ref).await
    }

    async fn deposit_address(&self, asset: &Asset, network: &str) -> GatewayResult<String> {
        self.addrs.get(&self.ctx, asset, network).await
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        deposits::deposit_status(&self.ctx, asset, tx_hash).await
    }

    /// Решение 150: депозиты EarlyConfirmed — торговать можно, вывести нельзя (deposits.rs).
    async fn withdraw_hold(&self, _quote: &str) -> GatewayResult<WithdrawHold> {
        deposits::withdraw_hold(&self.ctx).await
    }

    /// Решение 144: счета Kraken для панели «Балансы» — all_balances.rs (только чтение).
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        all_balances::read(&self.ctx).await
    }

    /// Решение 175: делистинги — «неизвестно»: в GET /0/public/AssetPairs только status (online, cancel_only, post_only, limit_only, reduce_only), времени делистинга нет.
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        Ok(crate::core::types::Delistings::unknown(
            "в GET /0/public/AssetPairs только status (online, cancel_only, post_only, limit_only, reduce_only), времени делистинга нет",
        ))
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx).await
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
}
