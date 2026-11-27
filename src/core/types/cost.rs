//! Файл: core/types/cost.rs
//! Что делает: затраты сделки (вид, сумма, в чём, шаг маршрута) и маршрут обмена на DEX (прямой пул или умный).
//! Кто вызывает: адаптеры (котировка, стоимость вывода), ядро (полный список затрат, решение 35),
//! интерфейс (расчёт кандидата).
//! От чего зависит: core/types/pool.rs, rust_decimal.
//! Настройки: нет.
//!
//! Правило: адаптер кладёт в котировку всё, что знает о своей стороне сделки; ядро добавляет своё
//! (вывод, возврат USDT, запас на движение цены) и переводит всё в USDT по живым ценам.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::pool::{PoolKind, PoolRef};

/// Вид затраты. Список — решение 35 и CLAUDE.md («все затраты»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CostKind {
    /// Торговая комиссия биржи (в той валюте, что списывает биржа).
    TradeFee,
    /// Комиссия пула (V2 — из настройки; V3 — fee(); V4 — текущая lp+protocol).
    PoolFee,
    /// Остаток от округления количества по шагу биржи.
    StepRemainder,
    /// Газ обмена (BNB).
    Gas,
    /// Газ разрешения approve/Permit2 — только в первый раз на монету (BNB).
    ApproveGas,
    /// Газ перевода монеты из кошелька на биржу (BNB).
    TransferGas,
    /// Газ обёртки нативной монеты в транзакции обмена (решение 62): WRAP_ETH при продаже BNB на DEX, UNWRAP_WETH
    /// при покупке (BNB) — сверх газа того же обмена токеном WBNB, измерено пробным обменом.
    WrapGas,
    /// Налог токена при покупке.
    BuyTax,
    /// Налог токена при продаже.
    SellTax,
    /// Налог токена при переводе.
    TransferTax,
    /// Решение 253, дополнение: недостача перевода по факту — потеря монеты при выводе с биржи, не объяснённая комиссией
    /// вывода и налогом перевода (core/withdraw_seen.rs, при transfer.seen_cost).
    TransferShortfall,
    /// Решение 413 (t198): удержание биржи-получателя при зачислении — на счёт пришло меньше записи депозита (живой
    /// случай WKC → MEXC 18.11: ровно −2 % налога токена). Замер или консервативное допущение — core/deposit_hold.rs.
    DepositHold,
    /// Влияние крючка группы B/C, измеренное пробным обменом.
    HookImpact,
    /// Чаевые каналу отправки (BNB).
    ChannelTip,
    /// Комиссия вывода биржи.
    WithdrawFee,
    /// Стоимость перегона USDT обратно (режим «в каждой сделке», решение 34) — считает ядро.
    UsdtReturn,
    /// Запас на движение цены за время перевода r = выручка·(1 − e^(−k·σ·√T)) (решение 390) — считает ядро.
    PriceMoveReserve,
}

/// Одна затрата.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub kind: CostKind,
    /// Сумма в единицах `asset`, неотрицательная.
    pub amount: Decimal,
    /// В чём: «USDT», «BNB» или тикер монеты.
    pub asset: String,
    /// true — уже учтено в средней цене котировки (показывается для прозрачности, повторно не вычитается);
    /// false — вычитается отдельно.
    pub in_price: bool,
    /// Шаг маршрута обмена, к которому относится затрата (решение 52): комиссия пула, проскальзывание,
    /// газ, налог — на каждом шаге «USDT → BNB», «BNB → монета». None — затрата всего маршрута или не обмена
    /// (комиссия биржи, вывод, возврат USDT, запас r). Прямой одношаговый маршрут — шаг 1.
    #[serde(default)]
    pub step: Option<RouteStep>,
    /// Решение 219: торговая комиссия, которую биржа берёт монетой скидки (GT у Gate, BNB у Binance), а сумма известна
    /// только в `asset` (USDT-эквивалент по ставке со скидкой: цены монеты скидки у адаптера нет). Тикер монеты
    /// скидки — из `asset` не списывается (не входит в предел списания котируемой монеты, не уменьшает количество
    /// монет), в прибыль идёт по сумме в `asset`. None — списывается сама `asset`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paid_in: Option<String>,
}

/// Шаг маршрута обмена: номер (с 1) и пара токенов «откуда → куда» (решение 52).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RouteStep {
    /// Номер шага по порядку, с 1.
    pub index: u32,
    /// Тикер того, что отдаём на шаге (например «USDT»).
    pub from: String,
    /// Тикер того, что получаем на шаге (например «BNB»).
    pub to: String,
    /// Адреса токенов в BSC (адрес — главный идентификатор); None — нативная монета или неизвестно.
    pub from_token: Option<alloy_primitives::Address>,
    pub to_token: Option<alloy_primitives::Address>,
}

impl Cost {
    /// Создаёт затрату без шага маршрута. Принимает: вид, сумму, актив, учтена ли в цене. Ошибок нет.
    pub fn new(kind: CostKind, amount: Decimal, asset: impl Into<String>, in_price: bool) -> Self {
        Self {
            kind,
            amount,
            asset: asset.into(),
            in_price,
            step: None,
            paid_in: None,
        }
    }

    /// Решение 219: та же затрата, но платится монетой скидки `coin` (см. поле paid_in). Ошибок нет.
    pub fn paid_with(mut self, coin: impl Into<String>) -> Self {
        self.paid_in = Some(coin.into());
        self
    }

    /// Привязывает затрату к шагу маршрута (адаптер DEX — для каждого обмена маршрута).
    /// Принимает: шаг. Возвращает: ту же затрату с шагом.
    pub fn with_step(mut self, step: RouteStep) -> Self {
        self.step = Some(step);
        self
    }
}

/// Режим обмена на DEX (решения 29, 38, 80).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteMode {
    /// Лучший одиночный пул, прямой контракт обмена.
    Direct,
    /// Маршрут через несколько пулов (хопы, доли), исполняет официальный Universal Router.
    Smart,
    /// Маршрут по локальному графу токенов (решение 80): любые площадки и промежуточные монеты, сплит между пулами.
    /// Исполнимость одной транзакцией — поле DexRoute.executable (решение 84).
    Graph,
    /// Одна подписанная RFQ. Идентификатор привязывает цену и вызов; подпись хранится только у адаптера в памяти.
    Rfq(alloy_primitives::B256),
    /// Индикативные уровни: до внешней финальной котировки отправлять нечего.
    RfqIndicative,
    /// Исполняемый снимок и сугубо индикативная книга цены строителя следующего блока.
    Slot(alloy_primitives::B256),
    SlotIndicative,
    /// Исполнимый снимок ончейн-книги, привязанный адаптером к параметрам ордера.
    Orderbook(alloy_primitives::B256),
    /// Глубина ончейн-книги для подбора размера до подтверждения условий исполнения.
    OrderbookIndicative,
}

/// Выбранная ставка опубликованного ABI роутера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouterFeeMode {
    Standard,
    Rebate,
    /// Официальный роутер проверяет валовую выплату ETH; аванс прибавлен к подписанному минимуму.
    NativeGrossMinimum,
    /// Защитный модуль ERC20 выбран по чистой выплате и закреплён до подписи.
    ProtectedWallet { target: alloy_primitives::Address },
    /// Ethereum Antfarm reimburses physical ATF from this trade's output, in the same call.
    AntfarmProceeds { target: alloy_primitives::Address, tier:u16, loan:bool },
    /// Семейство Carbon сохраняет прямой контроллер или выбранный модуль и физическую расчётную валюту.
    CarbonWallet {module:Option<alloy_primitives::Address>,native_cash:bool,#[serde(default)] allocation:Option<alloy_primitives::B256>},
    /// Нативная расчётная валюта; фиксируется вид минимума выбранного публичного ABI.
    NativeSettlement {
        gross_minimum: bool,
    },
}

/// Один пул на пути доли.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteHop {
    /// Вид пула у адаптера (id области «DEX»).
    pub family: String,
    pub kind: PoolKind,
    pub pool: PoolRef,
    /// Вариант одного и того же обмена, зафиксированный сравнением чистой выплаты.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_mode: Option<RouterFeeMode>,
}

/// Доля маршрута: часть входа идёт по цепочке пулов.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteLeg {
    /// Доля входа, 0–1. У прямого обмена — одна доля 1.
    pub share: Decimal,
    pub hops: Vec<RouteHop>,
    /// Ожидаемый выход доли по расчёту адаптера (в единицах выходного актива).
    pub expected_out: Decimal,
}

/// Маршрут обмена на DEX, на который рассчитана котировка; тот же маршрут уходит в TradeRequest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DexRoute {
    pub mode: RouteMode,
    pub legs: Vec<RouteLeg>,
    /// Блок, по состоянию которого посчитан маршрут.
    pub block: u64,
    /// Оценка газа обмена (единиц газа).
    pub gas_units: u64,
    /// Исполним ли маршрут одной транзакцией как есть (решение 84): прямая форма (один пул) или всё через один
    /// Universal Router (умный путь PancakeSwap). false — маршрут выходит за один роутер и нужен бандл свопов
    /// (решение 84, фаза исполнения): это сигнал/карточка, авто-сделкой пока не исполняется. Direct/Smart — всегда true.
    #[serde(default = "executable_default")]
    pub executable: bool,
    /// Причина неисполнимости как есть (для карточки сигнала, решение 74). None — исполним.
    #[serde(default)]
    pub exec_note: Option<String>,
}

/// По умолчанию маршрут исполним (старые записи без поля, Direct/Smart). Ошибок нет.
fn executable_default() -> bool {
    true
}
