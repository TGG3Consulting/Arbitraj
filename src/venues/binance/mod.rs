//! Файл: venues/binance/mod.rs
//! Что делает: адаптер биржи Binance (спот API + Wallet SAPI) — реализация контракта gateway/contract.rs (решение 160).
//! Всё общение с Binance — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «binance», feed.stale_after_ms; секреты binance.api_key, binance.api_secret.
//!
//! Только по официальной документации Binance (https://developers.binance.com/legacy-docs/binance-spot-api-docs/ и
//! Wallet SAPI https://developers.binance.com/docs/wallet/). Ссылки — в шапке каждого файла и в README.md папки.
//! Разбор перед подключением — docs/notes/binance-analysis-2026-11-05.md.

pub mod account;
pub mod account_update;
pub mod all_balances;
pub mod book;
pub mod ctx;
pub mod deposit_addr;
pub mod deposits;
pub mod depth;
pub mod discount_balance;
pub mod errors;
pub mod fee_rates;
pub mod feed;
pub mod fees;
pub mod fills;
pub mod fund_move;
pub mod key_check;
pub mod market;
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
pub mod tap;
pub mod tap_decode;
pub mod trade;
pub mod transfers;
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
    TradeRequest, TradeResult, Transfer, VenueId, WithdrawHold, WithdrawRequest,
};
use crate::gateway::contract::{AccountRole, ConnectionCheck, DexFamily, Venue, VenueKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::gateway::tap::{TapData, TapInfo, TapSink, TapStream};

use book::Books;
use ctx::Ctx;
use deposit_addr::DepositAddrs;
use fees::Fees;
use subs::{Parts, Subs};
use symbols::Symbols;
use wallet::Networks;

/// Адаптер Binance.
pub struct Binance {
    ctx: Ctx,
    /// Текущая подписка: задачи соединений, обновление справочника пар и комиссий (subs.rs).
    subs: Subs,
    symbols: Symbols,
    books: Books,
    fees: Fees,
    /// Справочник монет и сетей (wallet.rs).
    nets: Networks,
    /// Адреса депозита в памяти (deposit_addr.rs).
    addrs: DepositAddrs,
}

impl Binance {
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

    /// Ответ «не применимо» для функций только DEX. Ошибок нет.
    fn not_applicable(&self, what: &str) -> GatewayError {
        self.ctx.tag(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!("Binance — биржа: {what} к ней не применимо"),
        ))
    }
}

#[async_trait]
impl Venue for Binance {
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
            fees: self.fees.clone(),
        };
        self.subs.subscribe(&parts, assets, out).await
    }

    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        self.books
            .quote(&self.ctx, &self.fees, asset, side, qty)
            .await
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req).await
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

    async fn deposit_address(&self, asset: &Asset, network: &str) -> GatewayResult<String> {
        deposit_addr::deposit_address(&self.ctx, &self.addrs, asset, network).await
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        deposits::deposit_status(&self.ctx, asset, tx_hash).await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx).await
    }

    /// Решение 144: «Спот», «Фонд», «Все кошельки» параллельно (all_balances.rs), только чтение.
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        Ok(all_balances::all(&self.ctx).await)
    }

    /// Решение 150: депозиты со статусом 6 «credited but cannot withdraw» (deposits.rs).
    async fn withdraw_hold(&self, _quote: &str) -> GatewayResult<WithdrawHold> {
        deposits::withdraw_hold(&self.ctx).await
    }

    /// Решение 102: депозит, зачисленный на Funding, — на спот (fund_move.rs, venue.deposit_to_spot).
    /// Решение 174: роль счёта панели «Балансы» для кнопки «Продать» — названия те же, что в all_balances.rs: «Спот» и «Фонд» (Funding); deposit_to_trading — Funding → Spot (fund_move.rs).
    fn account_role(&self, account: &str) -> AccountRole {
        match account {
            "Спот" => AccountRole::Trading,
            "Фонд" => AccountRole::ToTrading,
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
        fund_move::to_spot(&self.ctx, asset, amount).await
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

    // Решение 125: запись окна — tap.rs (стакан — ws_conn.rs, сделки и изменения — feed.rs).
    async fn window_tap(&self, asset: &Asset, sink: TapSink) -> GatewayResult<TapInfo> {
        tap::start(&self.ctx, &self.symbols, asset, sink)
    }

    async fn window_untap(&self, asset: &Asset) -> GatewayResult<()> {
        tap::stop(&self.ctx, asset);
        Ok(())
    }

    fn decode_tap(
        &self,
        stream: TapStream,
        data: &TapData,
    ) -> GatewayResult<crate::core::types::TapDecoded> {
        tap::decode(stream, data)
    }
}
