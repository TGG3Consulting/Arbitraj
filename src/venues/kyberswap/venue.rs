//! Файл: venues/kyberswap/venue.rs
//! Что делает: адаптер DEX через агрегатор KyberSwap — реализация контракта gateway/contract.rs. Цены — из REST
//! агрегатора (узел для цен не нужен), исполнение — свопом через роутер агрегатора нашим кошельком и узлом BSC.
//! Пуловых/канальных/графовых концепций у агрегатора нет: dex_families и send_channels — пустые, остальные пуловые
//! методы — умолчания контракта (NotApplicable).
//! Кто вызывает: main.rs (через mod.rs: создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, core/types.
//! Настройки: venue.* [kyberswap], kyber.* (docs/SETTINGS.md).

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

use super::Kyberswap;
use super::{balances, coins, connection, market, quote, signer, trade, transfer};

#[async_trait]
impl Venue for Kyberswap {
    /// Идентификатор площадки (`kyberswap`). Ошибок нет.
    fn id(&self) -> VenueId {
        self.p.ctx.venue()
    }

    /// Вид площадки — DEX. Ошибок нет.
    fn kind(&self) -> VenueKind {
        VenueKind::Dex
    }

    /// «Проверить подключение»: API агрегатора, узел BSC, кошелёк — connection.rs. Ошибок нет: причина — в ответе.
    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        Ok(connection::check(&self.p).await)
    }

    /// Монеты активной подписки в сети BSC — coins.rs. Ошибки: NotConfigured (venue.bsc_network).
    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        coins::networks(&self.p)
    }

    /// Поллинг квот агрегатора → события ядру (Pool, Invalidated, CoinCheck) — market.rs. Прежняя задача останавливается.
    /// Ошибки: NotConfigured (kyber.quote_poll_ms, kyber.quote_concurrency, kyber.probe_usdt).
    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        market::subscribe(&self.p, assets, out).await
    }

    /// Котировка на объём из GET /routes — quote.rs. Ошибки: NoRoute, NotConfigured, узел (decimals), сеть.
    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        quote::quote(&self.p, asset, side, qty).await
    }

    /// Своп через роутер агрегатора (свежий маршрут, build, approve, отправка, факт по логам) — trade.rs.
    /// Ошибки: NoRoute, QuoteMismatch, SwapReverted / TxNotIncluded / OutcomeUnknown (с фактом транзакции).
    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        trade::trade(&self.p, req).await
    }

    /// Перевод монеты из кошелька на адрес (ERC-20 transfer) — transfer.rs. Ошибки: InvalidAddress, сеть, узел,
    /// WithdrawRejected / TxNotIncluded (с фактом транзакции).
    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        transfer::withdraw(&self.p, req).await
    }

    /// Статус перевода из кошелька по хешу — transfer.rs. Ошибки: узел, хеш не 32 байта, BadResponse.
    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        transfer::withdrawal_status(&self.p, venue_ref).await
    }

    /// Адрес кошелька для депозита (любая монета принимается на один адрес). Ошибки: ключ кошелька.
    async fn deposit_address(&self, _asset: &Asset, _network: &str) -> GatewayResult<String> {
        Ok(signer::address(&self.p.ctx)?.to_string())
    }

    /// Статус зачисления монеты в кошелёк по хешу — transfer.rs. Ошибки: узел, NotConfigured, BadResponse.
    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        transfer::deposit_status(&self.p, asset, tx_hash).await
    }

    /// Балансы кошелька: BNB и известные монеты — balances.rs. Ошибки: узел, ключ.
    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        balances::all(&self.p).await
    }

    /// Стоимость перевода: газ и (при kyber.assume_zero_transfer_tax) нулевой налог — transfer.rs.
    /// Ошибки: CoinNotChecked (налог не разрешён считать нулевым), сеть, узел.
    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        transfer::withdraw_cost(&self.p, asset, network, amount).await
    }

    /// Перепроверка налогов монет: у агрегатора пробы налогов нет (amountOut их уже учитывает) — NotApplicable.
    async fn recheck_coins(&self, _assets: Vec<Asset>) -> GatewayResult<()> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у агрегатора нет отдельной проверки налогов монет (учтены в котировке маршрута)",
        ))
    }

    /// Виды пулов DEX: у агрегатора своих пулов/видов нет (он сам обходит все площадки) — пусто. Ошибок нет.
    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        Ok(Vec::new())
    }

    /// Каналы отправки: своп уходит прямо через узел (eth_sendRawTransaction), каналов нет — пусто. Ошибок нет.
    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        Ok(Vec::new())
    }

    /// «Проверить подключение» канала: каналов у площадки нет. Ошибки: NotConfigured (неизвестный канал).
    async fn check_send_channel(&self, id: &str) -> GatewayResult<ConnectionCheck> {
        Err(GatewayError::new(
            ErrorCode::NotConfigured,
            Some(self.id()),
            format!("у агрегатора KyberSwap нет каналов отправки — канал «{id}» неизвестен"),
        ))
    }
}
