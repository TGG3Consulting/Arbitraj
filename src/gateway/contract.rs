//! Файл: gateway/contract.rs
//! Что делает: контракт адаптера площадки — набор функций, который обязан реализовать
//! каждый адаптер (биржа или DEX). Ядро видит площадки только через этот контракт.
//! Кто вызывает: шлюз (gateway/registry.rs). Реализуют: адаптеры в venues/.
//! От чего зависит: core/types, gateway/errors, gateway/events.
//! Настройки: у каждого адаптера свои (venue.<id>.*).
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).
//! Подробности — ARCHITECTURE.md, раздел «Контракт адаптера», и docs/ADAPTER_TEMPLATE.md.

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::core::types::{
    Asset, Balance, CoinNetworkInfo, Quote, Side, TradeRequest, TradeResult, Transfer, VenueId,
    WithdrawRequest,
};

use super::errors::GatewayResult;
use super::events::MarketEvent;

/// Вид площадки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VenueKind {
    /// Централизованная биржа (стакан, ордера, вывод и депозит).
    Cex,
    /// DEX в сети (пулы, свопы через свой кошелёк).
    Dex,
}

/// Результат кнопки «Проверить подключение».
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionCheck {
    /// true — «Подключено».
    pub ok: bool,
    /// Точная причина сбоя или краткое описание успеха (например, права ключа).
    pub details: String,
}

/// Контракт адаптера. Все функции возвращают данные в формате ядра.
/// Ошибки площадки адаптер переводит в коды gateway/errors.rs.
#[async_trait]
pub trait Venue: Send + Sync {
    /// Идентификатор площадки. Не может ошибиться.
    fn id(&self) -> VenueId;

    /// Вид площадки. Не может ошибиться.
    fn kind(&self) -> VenueKind;

    /// Проверка подключения для кнопки в интерфейсе: ключи, права, доступность.
    /// Возвращает: ConnectionCheck. Ошибка — только если проверку невозможно выполнить.
    async fn check_connection(&self) -> GatewayResult<ConnectionCheck>;

    /// Справочник: монеты площадки и их сети (ввод/вывод, комиссия вывода, адрес контракта).
    /// Возвращает: список по всем монетам и сетям. Ошибки: связь, ключ, разбор ответа.
    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>>;

    /// Подписка на рыночные данные (стаканы или пулы) по списку монет.
    /// Принимает: монеты и канал, куда адаптер шлёт события постоянно, пока жив канал.
    /// Возвращает: Ok после успешной подписки. Ошибки: связь, лимиты подписок.
    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()>;

    /// Котировка на объём по текущим данным (стакан по уровням, пул по содержимому).
    /// Принимает: монету, сторону, количество. Возвращает: Quote. Ошибки: нет данных, данные устарели.
    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote>;

    /// Покупка или продажа: ордер на бирже или своп на DEX.
    /// Возвращает: фактический результат. Ошибки: отклонено, не исполнено, баланс, сеть.
    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult>;

    /// Вывод монеты с площадки на адрес. Для DEX — перевод из своего кошелька.
    /// Возвращает: созданный перевод. Ошибки: адрес, права, баланс, вывод закрыт.
    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer>;

    /// Текущий статус вывода по идентификатору площадки.
    /// Возвращает: перевод с актуальным статусом. Ошибки: не найден, связь.
    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer>;

    /// Адрес для депозита монеты в указанной сети.
    /// Возвращает: адрес. Ошибки: ввод закрыт, сеть не поддерживается.
    async fn deposit_address(&self, asset: &Asset, network: &str) -> GatewayResult<String>;

    /// Статус депозита по хешу транзакции в сети.
    /// Возвращает: перевод с актуальным статусом. Ошибки: не найден, связь.
    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer>;

    /// Балансы всех активов на площадке (или в кошельке для DEX).
    /// Ошибки: ключ, связь.
    async fn balances(&self) -> GatewayResult<Vec<Balance>>;
}
