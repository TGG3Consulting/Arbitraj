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
//! Общее состояние — parts.rs; здесь только функции контракта. Срочность запросов к узлу (решение 61, node/priority.rs):
//! сделка, вывод, сверки перевода и стоимость вывода — Trade; котировка — Check; остальное — фон.

pub mod check;
pub mod coins;
pub mod connection;
pub mod connection_extra;
pub mod ctx;
pub mod directory;
pub mod errors;
pub mod exec;
pub mod math;
pub mod node;
pub mod num;
pub mod parts;
pub mod pools;
pub mod quote;
pub mod resubscribe;
pub mod send;
pub mod tokens;
pub mod wallet;

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    Asset, Balance, CoinNetworkInfo, Cost, PoolDirectory, Quote, SendChannelInfo, Side,
    TradeRequest, TradeResult, Transfer, VenueId, WalletQueue, WithdrawRequest,
};
use crate::gateway::contract::{ConnectionCheck, DexFamily, Venue, VenueKind};
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;

use node::priority::{Class, scope};
use parts::Parts;

/// Адаптер DEX.
pub struct BscDex {
    p: Parts,
    /// Подписка на рынок: задачи, последний список, пересборка при изменении настроек промежуточных монет.
    subs: resubscribe::Subs,
}

impl BscDex {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: ctx::Ctx) -> Self {
        Self {
            p: Parts::new(ctx),
            subs: resubscribe::Subs::default(),
        }
    }

    /// Пересобирать подписку при изменении dex.intermediate_* (решение 52) — resubscribe.rs. Принимает: приёмник
    /// изменений настроек. Возвращает: задачу наблюдателя. Ошибок нет.
    pub fn watch_settings(
        &self,
        rx: tokio::sync::broadcast::Receiver<crate::settings::live::Change>,
    ) -> tokio::task::JoinHandle<()> {
        self.subs.watch(self.p.clone(), rx)
    }
}

#[async_trait]
impl Venue for BscDex {
    /// Идентификатор площадки (`bsc_dex`). Ошибок нет.
    fn id(&self) -> VenueId {
        self.p.ctx.venue()
    }

    /// Вид площадки — DEX. Ошибок нет.
    fn kind(&self) -> VenueKind {
        VenueKind::Dex
    }

    /// «Проверить подключение»: узел HTTP (eth_chainId = 56, eth_blockNumber), узел WebSocket (newHeads), кошелёк,
    /// eth_simulateV1 — connection.rs (execution-apis src/eth, коммит 16caee27). Ошибок нет: причина — в ответе.
    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        Ok(connection::check(&self.p.ctx).await)
    }

    /// Монеты DEX с достоверными пулами (адрес токена — из пула, сеть BSC) — coins.rs. Ошибки: NotConfigured.
    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        coins::networks(&self.p.ctx, &self.p.registry)
    }

    /// Подписка на рынок: опрос блоков (решение 57), пулы монет (eth_subscribe logs, execution-apis subscribe.yaml), новые пулы,
    /// циклы проверок; события — в `out` (pools/market.rs). Прежние задачи подписки останавливаются.
    /// Ошибки: NotConfigured (venue.quote_asset, dex.*), узел.
    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        self.subs.subscribe(&self.p, assets, out).await
    }

    /// Котировка на объём по пулам (локальная математика по официальным библиотекам — quote/, math/).
    /// Ошибки: StaleData, CoinNotChecked / CoinCheckFailed, NoRoute, NotConfigured, узел.
    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        // Котировка — на каждый тик монеты и в предпроверке ядра: класс Check (выше фона, наравне с проверками).
        scope(Class::Check, self.p.quoting().quote(asset, side, qty)).await
    }

    /// Обмен через Universal Router своей DEX (прямой) или PancakeSwap Infinity (умный) — exec/trade.rs; источники —
    /// research/flow/bsc_dex.md, раздел 5, bsc_smart_router.md. Ошибки: см. exec/trade.rs (с фактом транзакции).
    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        let (q, env) = (self.p.quoting(), self.p.env());
        scope(
            Class::Trade,
            exec::trade::trade(&q, &env, &self.p.nonces, req),
        )
        .await
    }

    /// Перевод монеты из кошелька на адрес (ERC-20 transfer, EIP-20) — wallet/transfer.rs.
    /// Ошибки: InvalidAddress, NotConfigured, каналы, TxNotIncluded / WithdrawRejected (с фактом транзакции).
    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        let env = self.p.env();
        scope(
            Class::Trade,
            wallet::transfer::withdraw(&env, &self.p.tokens, &self.p.nonces, req),
        )
        .await
    }

    /// Статус перевода из кошелька по квитанции (execution-apis transaction.yaml#L127-L165) — wallet/transfer_status.rs.
    /// Ошибки: узел, хеш не 32 байта, BadResponse.
    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        let w = wallet::signer::address(&self.p.ctx)?;
        let work =
            wallet::transfer_status::withdrawal_status(&self.p.ctx, &self.p.tokens, venue_ref, w);
        scope(Class::Trade, work).await
    }

    /// Адрес кошелька; заодно регистрирует ожидаемое поступление монеты (после конца идущего обмена с ней на выходе) —
    /// wallet/inflows.rs. Ошибки: ключ кошелька, NotConfigured (у монеты нет адреса в BSC).
    async fn deposit_address(&self, asset: &Asset, _network: &str) -> GatewayResult<String> {
        let w = wallet::signer::address(&self.p.ctx)?;
        let token = wallet::transfer::token(&self.p.ctx, asset)?;
        self.p.inflows.expect(token).await;
        Ok(w.to_string())
    }

    /// Статус зачисления; любое завершение снимает ожидаемое поступление — wallet/deposit.rs.
    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        scope(
            Class::Trade,
            wallet::deposit::status(&self.p, asset, tx_hash),
        )
        .await
    }

    /// Вывод на кошелёк не состоялся — ожидание поступления снято (wallet/deposit.rs). Ошибок нет.
    async fn deposit_abandoned(&self, asset: &Asset) -> GatewayResult<()> {
        wallet::deposit::abandoned(&self.p, asset).await;
        Ok(())
    }

    /// Очередь транзакций кошелька (wallet/nonce.rs). Ошибок нет.
    async fn wallet_queue(&self) -> GatewayResult<WalletQueue> {
        Ok(wallet::deposit::queue(&self.p).await)
    }

    /// «Снять очередь после ручной проверки» — wallet/deposit.rs. Ошибки: NotApplicable — очередь не остановлена.
    async fn release_wallet_queue(&self, by: &str, pending_ok: bool) -> GatewayResult<WalletQueue> {
        scope(
            Class::Trade,
            wallet::deposit::release(&self.p, by, pending_ok),
        )
        .await
    }

    /// Балансы кошелька: BNB (eth_getBalance) и известные токены (balanceOf, EIP-20) — wallet/balances.rs. Ошибки: узел, ключ.
    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        wallet::balances::all(&self.p.ctx, &self.p.tokens).await
    }

    /// Стоимость перевода: газ (пробный перевод × цена газа) и налог на перевод — wallet/transfer.rs.
    /// Ошибки: CoinNotChecked / CoinCheckFailed, узел.
    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        let env = self.p.env();
        scope(
            Class::Trade,
            wallet::transfer::cost(&env, &self.p.checks, asset, network, amount),
        )
        .await
    }

    /// Кнопка «Перепроверить»: налоги, крючки и сверка с квотером — только этих монет (check/recheck.rs; итоги —
    /// события CoinCheck / PoolCheck). Ошибок нет.
    async fn recheck_coins(&self, assets: Vec<Asset>) -> GatewayResult<()> {
        let tickers: Vec<String> = assets.into_iter().map(|a| a.ticker).collect();
        self.p.wake.coins(&tickers);
        Ok(())
    }

    /// DEX с видом пулов из таблицы сопоставления (id области «DEX» = id DEX индексатора) — pools/families.rs. Ошибок нет.
    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        Ok(pools::families::for_ui(&self.p.ctx))
    }

    /// Справочник пулов от индексатора (решение 58) — directory/. Ошибок нет: нет данных — пустые списки и причина.
    async fn pool_directory(&self) -> GatewayResult<PoolDirectory> {
        Ok(directory::snapshot(&self.p.ctx))
    }

    /// Каналы отправки (id области «канал») — send/channels.rs. Ошибок нет.
    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        Ok(send::channels::ALL.iter().map(|c| c.info()).collect())
    }

    /// «Проверить подключение» канала: пустая транзакция — канал отвечает ошибкой JSON-RPC (send/check.rs; документация
    /// канала — в шапке его файла). Ошибки: неизвестный id.
    async fn check_send_channel(&self, id: &str) -> GatewayResult<ConnectionCheck> {
        send::check::check(&self.p.ctx, id).await
    }
}
