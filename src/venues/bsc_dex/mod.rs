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
//! Общее состояние — parts.rs; здесь только функции контракта.

pub mod check;
pub mod coins;
pub mod connection;
pub mod ctx;
pub mod errors;
pub mod exec;
pub mod math;
pub mod node;
pub mod num;
pub mod parts;
pub mod pools;
pub mod quote;
pub mod send;
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
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;

use parts::Parts;

/// Адаптер DEX.
pub struct BscDex {
    p: Parts,
    /// Задачи текущей подписки: при новой подписке старые останавливаются (без дублей соединений).
    tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl BscDex {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: ctx::Ctx) -> Self {
        Self {
            p: Parts::new(ctx),
            tasks: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Общее состояние (для проверок и тестов). Ошибок нет.
    pub fn parts(&self) -> &Parts {
        &self.p
    }
}

#[async_trait]
impl Venue for BscDex {
    fn id(&self) -> VenueId {
        self.p.ctx.venue()
    }

    fn kind(&self) -> VenueKind {
        VenueKind::Dex
    }

    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        Ok(connection::check(&self.p.ctx).await)
    }

    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        coins::networks(&self.p.ctx, &self.p.registry)
    }

    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        let pools = self.p.pools(out);
        let new = pools::market::subscribe(&pools, &self.p.scanned, &self.p.wake, assets).await?;
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        for old in tasks.drain(..) {
            old.abort();
        }
        *tasks = new;
        Ok(())
    }

    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        self.p.quoting().quote(asset, side, qty).await
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        exec::trade::trade(&self.p.quoting(), &self.p.env(), &self.p.nonces, req).await
    }

    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        wallet::transfer::withdraw(&self.p.env(), &self.p.tokens, &self.p.nonces, req).await
    }

    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        let w = wallet::signer::address(&self.p.ctx)?;
        wallet::transfer_status::withdrawal_status(&self.p.ctx, &self.p.tokens, venue_ref, w).await
    }

    async fn deposit_address(&self, _asset: &Asset, _network: &str) -> GatewayResult<String> {
        Ok(wallet::signer::address(&self.p.ctx)?.to_string())
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        let w = wallet::signer::address(&self.p.ctx)?;
        wallet::transfer_status::deposit_status(&self.p.ctx, &self.p.tokens, asset, tx_hash, w)
            .await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        wallet::balances::all(&self.p.ctx, &self.p.tokens).await
    }

    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        wallet::transfer::cost(&self.p.env(), &self.p.checks, asset, network, amount).await
    }

    async fn recheck_coins(&self, _assets: Vec<Asset>) -> GatewayResult<()> {
        // Циклы проверяют все монеты подписки; кнопка будит их сразу (итоги — событиями CoinCheck / PoolCheck).
        self.p.wake.notify_waiters();
        Ok(())
    }

    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        Ok(pools::families::for_ui())
    }

    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        Ok(send::channels::ALL.iter().map(|c| c.info()).collect())
    }

    async fn check_send_channel(&self, id: &str) -> GatewayResult<ConnectionCheck> {
        send::check::check(&self.p.ctx, id).await
    }
}
