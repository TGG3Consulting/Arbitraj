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

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req).await
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
}
