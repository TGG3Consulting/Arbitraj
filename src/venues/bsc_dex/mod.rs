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

pub mod check;
pub mod coins;
pub mod connection;
pub mod ctx;
pub mod errors;
pub mod exec;
pub mod math;
pub mod node;
pub mod num;
pub mod pools;
pub mod quote;
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

use check::state::Checks;
use ctx::Ctx;
use node::heads::Heads;
use pools::discover::Scanned;
use pools::listen::Pools;
use pools::registry::Registry;
use quote::Quoting;
use quote::gas::GasPrice;
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
    /// Пулы в памяти.
    registry: Registry,
    /// Прочитанная история событий создания пулов.
    scanned: Scanned,
    /// Итоги проверок монет и пулов.
    checks: Checks,
    /// Цена газа (кэш на блок).
    gas: GasPrice,
    /// Сигнал «перепроверить сейчас» циклам проверок (recheck_coins).
    wake: std::sync::Arc<tokio::sync::Notify>,
    /// Задачи текущей подписки: при новой подписке старые останавливаются.
    tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl BscDex {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            heads: Heads::default(),
            tokens: Tokens::default(),
            nonces: Nonces::default(),
            registry: Registry::default(),
            scanned: Scanned::default(),
            checks: Checks::default(),
            gas: GasPrice::default(),
            wake: std::sync::Arc::new(tokio::sync::Notify::new()),
            tasks: std::sync::Mutex::new(Vec::new()),
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

    /// Окружение котировки. Ошибок нет.
    pub fn quoting(&self) -> Quoting {
        Quoting {
            ctx: self.ctx.clone(),
            reg: self.registry.clone(),
            checks: self.checks.clone(),
            tokens: self.tokens.clone(),
            heads: self.heads.clone(),
            gas: self.gas.clone(),
        }
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
        coins::networks(&self.ctx, &self.registry)
    }

    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        let p = Pools {
            ctx: self.ctx.clone(),
            reg: self.registry.clone(),
            tokens: self.tokens.clone(),
            out,
            checks: self.checks.clone(),
            heads: self.heads.clone(),
        };
        let new = pools::market::subscribe(&p, &self.scanned, &self.wake, assets).await?;
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        for old in tasks.drain(..) {
            old.abort();
        }
        *tasks = new;
        Ok(())
    }

    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        self.quoting().quote(asset, side, qty).await
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
        // Циклы проверяют все монеты подписки; кнопка будит их сразу (итоги — событиями CoinCheck / PoolCheck).
        self.wake.notify_waiters();
        Ok(())
    }

    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        Ok(pools::families::for_ui())
    }

    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        Err(self.not_yet("каналы отправки", "1d"))
    }

    async fn check_send_channel(&self, _id: &str) -> GatewayResult<ConnectionCheck> {
        Err(self.not_yet("проверка канала отправки", "1d"))
    }
}
