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
    Asset, Balance, CoinNetworkInfo, Cost, PoolDirectory, PoolKind, Quote, SendChannelInfo, Side,
    TradeRequest, TradeResult, Transfer, VenueId, WalletQueue, WithdrawRequest,
};

use super::errors::{ErrorCode, GatewayError, GatewayResult};
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

/// Вид пула, который умеет адаптер DEX: id области настроек «DEX» и что это.
/// Нужен интерфейсу, чтобы показать настройки dex.* по каждой DEX и виду пула.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DexFamily {
    /// Имя DEX. С решения 58 каждая DEX индексатора — своя строка таблицы сопоставления: dex = family = id DEX у
    /// индексатора (например «pancakeswap-v3-bsc»).
    pub dex: String,
    /// id области «DEX» для dex.kind, dex.pool_enabled и адресов контрактов (id DEX у индексатора).
    pub family: String,
    pub kind: PoolKind,
    /// Название для интерфейса.
    pub title: String,
    /// Ссылки на официальную документацию (адреса контрактов).
    pub docs: Vec<String>,
}

/// Контракт адаптера. Все функции возвращают данные в формате ядра.
/// Ошибки площадки адаптер переводит в коды gateway/errors.rs.
/// Функция, которая к площадке не применима, честно отвечает ошибкой NotApplicable —
/// никаких выдуманных данных (CLAUDE.md, «Никаких моков и заглушек»).
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

    /// Сколько стоит вывести монету с площадки: биржа — комиссия вывода из её API;
    /// DEX — газ перевода из кошелька (BNB) и налог токена на перевод.
    /// Принимает: монету, сеть, сумму. Возвращает: список затрат (Cost).
    /// Ошибки: нет данных, сеть не найдена, монета не проверена (DEX: CoinNotChecked).
    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        amount: Decimal,
    ) -> GatewayResult<Vec<Cost>>;

    /// Перепроверить налоги монет и крючки их пулов сейчас, не дожидаясь цикла (кнопка в интерфейсе).
    /// Результаты приходят событиями CoinCheck / PoolCheck в канал subscribe_market.
    /// Ошибки: NotApplicable — у площадки нет такой проверки (биржа).
    async fn recheck_coins(&self, assets: Vec<Asset>) -> GatewayResult<()>;

    /// Виды пулов, которые умеет адаптер DEX (для настроек dex.* в интерфейсе).
    /// Ошибки: NotApplicable — не DEX.
    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>>;

    /// Каналы отправки транзакций, которые умеет адаптер (решение 41).
    /// Ошибки: NotApplicable — у площадки нет каналов отправки (биржа).
    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>>;

    /// «Проверить подключение» одного канала отправки: адрес, ключ, сеть.
    /// Возвращает: ConnectionCheck. Ошибки: NotApplicable (биржа); неизвестный канал — NotConfigured.
    async fn check_send_channel(&self, id: &str) -> GatewayResult<ConnectionCheck>;

    /// Очередь транзакций кошелька (DEX, решение 45): свободна / ждёт итога / остановлена до ручной проверки.
    /// Ошибки: NotApplicable — у площадки нет кошелька (биржа).
    async fn wallet_queue(&self) -> GatewayResult<WalletQueue> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет очереди кошелька",
        ))
    }

    /// «Снять очередь после ручной проверки»: очередь, остановленная с неизвестным исходом, снова свободна.
    /// Принимает: кто снимает (для журнала); подтверждено ли снятие при ещё не занятом nonce (висящая транзакция
    /// может пройти позже). Ошибки: NotApplicable (биржа; очередь не остановлена); NonceConflict — nonce ещё не занят,
    /// а подтверждения нет.
    async fn release_wallet_queue(&self, by: &str, pending_ok: bool) -> GatewayResult<WalletQueue> {
        let _ = (by, pending_ok);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет очереди кошелька",
        ))
    }

    /// Справочник пулов от индексатора (решение 58): DEX индексатора, пулы монет и что с каждым сделал адаптер
    /// (взят, DEX не подключена, проверка на узле не прошла). Ошибки: NotApplicable — не DEX.
    async fn pool_directory(&self) -> GatewayResult<PoolDirectory> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет справочника пулов",
        ))
    }

    /// Ядро больше не ждёт депозит монеты на эту площадку (вывод на её адрес не состоялся или прерван):
    /// площадка снимает ожидание поступления. Ошибок нет у тех, кому это не нужно (биржа — ничего не делает).
    async fn deposit_abandoned(&self, asset: &Asset) -> GatewayResult<()> {
        let _ = asset;
        Ok(())
    }
}
