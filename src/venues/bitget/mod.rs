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
pub mod ctx;
pub mod deposit_addr;
pub mod deposits;
pub mod depth;
pub mod errors;
pub mod fees;
pub mod fund_move;
pub mod market;
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
            return book::priced(&self.ctx, &self.fees, &symbol, asset, side, qty, &book);
        }
        self.books
            .quote(&self.ctx, &self.fees, &symbol, asset, side, qty)
            .await
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req).await
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

    /// Решение 175: делистинги — «неизвестно»: поле offTime справочника GET /api/v2/spot/public/symbols не сверено с официальной документацией (описание поля не прочитано в сессии решения 175) — до проверки не используется.
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        Ok(crate::core::types::Delistings::unknown(
            "поле offTime справочника GET /api/v2/spot/public/symbols не сверено с официальной документацией (описание поля не прочитано в сессии решения 175) — до проверки не используется",
        ))
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
}
