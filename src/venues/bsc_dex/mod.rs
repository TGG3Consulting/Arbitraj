//! Файл: venues/bsc_dex/mod.rs
//! Что делает: адаптер DEX в сети BSC (PancakeSwap, Uniswap; один кошелёк, один узел) — реализация контракта
//! gateway/contract.rs. Всё общение с узлом и контрактами — внутри этой папки; наружу — типы ядра и единые ошибки.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: docs/SETTINGS.md — «Узел BSC», «DEX», «Отправка транзакций», venue.* и feed.* с областью bsc_dex.
//!
//! Только по официальной документации и данным самого блокчейна (CLAUDE.md, решение 23). Источники —
//! README.md этой папки и шапки файлов. Разработка по шагам Issue #7: 1a (узел, кошелёк, проверка подключения),
//! 1b (пулы), 1c (математика, котировка, проверки), 1d (исполнение, каналы отправки), 1e (умный режим).
//! Функции следующих шагов пока честно отвечают ошибкой Internal «ещё не сделано» — без выдуманных данных.

pub mod connection;
pub mod ctx;
pub mod errors;
pub mod node;
pub mod num;
pub mod tokens;
pub mod wallet;

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

use ctx::Ctx;
use node::heads::Heads;
use tokens::Tokens;
use wallet::nonce::Nonces;

/// Адаптер DEX.
pub struct BscDex {
    ctx: Ctx,
    /// Последний блок сети (newHeads).
    heads: Heads,
    /// Монеты из подписки ядра и их decimals.
    tokens: Tokens,
    /// Строгая очередь транзакций кошелька (решение 45).
    nonces: Nonces,
}

impl BscDex {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            heads: Heads::default(),
            tokens: Tokens::default(),
            nonces: Nonces::default(),
        }
    }

    /// Последний блок сети (для котировок и трекера транзакций). Ошибок нет.
    pub fn heads(&self) -> &Heads {
        &self.heads
    }

    /// Очередь транзакций кошелька (для exec/*, wallet/*). Ошибок нет.
    pub fn nonces(&self) -> &Nonces {
        &self.nonces
    }

    /// Ответ для функций, которые появятся в следующих шагах Issue #7.
    /// Принимает: что именно и в каком шаге. Возвращает: ошибку Internal с понятным текстом.
    fn not_yet(&self, what: &str, step: &str) -> GatewayError {
        self.ctx.err(
            ErrorCode::Internal,
            format!("DEX: {what} ещё не сделано (шаг {step}, Issue #7) — торговли по DEX нет"),
        )
    }
}

#[async_trait]
impl Venue for BscDex {
    fn id(&self) -> VenueId {
        self.ctx.venue()
    }

    fn kind(&self) -> VenueKind {
        VenueKind::Dex
    }

    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        Ok(connection::check(&self.ctx).await)
    }

    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        Err(self.not_yet("справочник монет", "1b"))
    }

    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        _out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        self.tokens.set(&assets);
        Err(self.not_yet("подписка на пулы", "1b"))
    }

    async fn quote(&self, _asset: &Asset, _side: Side, _qty: Decimal) -> GatewayResult<Quote> {
        Err(self.not_yet("котировка", "1c"))
    }

    async fn trade(&self, _req: TradeRequest) -> GatewayResult<TradeResult> {
        Err(self.not_yet("обмен", "1d"))
    }

    async fn withdraw(&self, _req: WithdrawRequest) -> GatewayResult<Transfer> {
        Err(self.not_yet("перевод из кошелька", "1d"))
    }

    async fn withdrawal_status(&self, _venue_ref: &str) -> GatewayResult<Transfer> {
        Err(self.not_yet("статус перевода", "1d"))
    }

    async fn deposit_address(&self, _asset: &Asset, _network: &str) -> GatewayResult<String> {
        Ok(wallet::signer::address(&self.ctx)?.to_string())
    }

    async fn deposit_status(&self, _asset: &Asset, _tx_hash: &str) -> GatewayResult<Transfer> {
        Err(self.not_yet("статус зачисления в кошелёк", "1d"))
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        wallet::balances::all(&self.ctx, &self.tokens).await
    }

    async fn withdraw_cost(
        &self,
        _asset: &Asset,
        _network: &str,
        _amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        Err(self.not_yet("стоимость перевода", "1d"))
    }

    async fn recheck_coins(&self, _assets: Vec<Asset>) -> GatewayResult<()> {
        Err(self.not_yet("проверка налогов и крючков", "1c"))
    }

    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        Err(self.not_yet("список видов пулов", "1b"))
    }

    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        Err(self.not_yet("каналы отправки", "1d"))
    }

    async fn check_send_channel(&self, _id: &str) -> GatewayResult<ConnectionCheck> {
        Err(self.not_yet("проверка канала отправки", "1d"))
    }
}
