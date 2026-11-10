//! Файл: core/types/trade.rs
//! Что делает: типы торговли — сторона сделки, котировка на объём, запрос и результат покупки/продажи.
//! Кто вызывает: ядро (расчёт, исполнение), шлюз и адаптеры через контракт.
//! От чего зависит: core/types/market.rs, cost.rs (затраты, маршрут DEX), send.rs (транзакция в сети), fee.rs (часть
//! комиссии ордера).
//! Настройки: нет.
//!
//! Покупка и продажа описываются одинаково для биржи и DEX:
//! у биржи это лимитный ордер, у DEX — своп с минимально допустимым результатом.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::cost::{Cost, DexRoute};
use super::market::{Asset, VenueId};
use super::send::ChainTx;

/// Часть фактической комиссии — core/types/fee.rs (вынесена: файл вырос; путь types::trade::Fee сохранён).
pub use super::fee::Fee;

/// Сторона сделки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

/// Котировка на объём: сколько заплатим или получим за заданное количество монет
/// с учётом реальной ликвидности (стакан — по уровням, пул — по его содержимому).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub venue: VenueId,
    pub asset: Asset,
    pub side: Side,
    /// Запрошенное количество монет.
    pub qty: Decimal,
    /// Количество, которое площадка реально может исполнить (меньше qty, если не хватает ликвидности).
    pub fillable_qty: Decimal,
    /// Средняя цена исполнения в USDT. Проскальзывание уже внутри (уровни стакана / математика пула);
    /// у DEX внутри и всё, что помечено в costs как in_price (комиссия пула, налог, влияние крючка).
    pub avg_price: Decimal,
    /// Затраты этой стороны сделки, известные площадке (CostKind): торговая комиссия, комиссия пула,
    /// газ, approve, чаевые канала, налоги, влияние крючка, остаток от округления.
    pub costs: Vec<Cost>,
    /// DEX: маршрут, на который посчитана котировка (прямой пул или умный). Биржа — None.
    pub route: Option<DexRoute>,
    /// DEX, граф-режим (решения 80, 84): лучший НАЙДЕННЫЙ маршрут, если он лучше того, что идёт на сделку (`route`),
    /// но неисполним как есть (мульти-роутер) — для карточки сигнала. None — лучший найденный и есть `route`.
    #[serde(default)]
    pub signal_route: Option<DexRoute>,
    /// DEX, режим «лучший из двух» (решение 123, exec.dex_best_of): кто дал финальную котировку и почему — текст
    /// адаптера («KyberSwap 24.15 vs свой 24.21 USDT за 10 CAKE → свой (+0.06)»). Ядро его не разбирает, только
    /// показывает («Кандидаты», план и хронология сделки). None — выбора не было.
    #[serde(default)]
    pub pick: Option<String>,
    /// Время, на которое рассчитана котировка.
    pub at: DateTime<Utc>,
}

/// Запрос на покупку или продажу.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRequest {
    pub venue: VenueId,
    pub asset: Asset,
    pub side: Side,
    /// Количество монет.
    pub qty: Decimal,
    /// Худшая допустимая средняя цена: для биржи — цена лимитного ордера,
    /// для DEX — из неё считается минимально допустимый результат свопа.
    pub worst_price: Decimal,
    /// Идентификатор сделки ядра, для связи с журналом.
    pub deal_id: String,
    /// DEX: маршрут из котировки — адаптер перепроверяет его (eth_call) перед отправкой. Биржа — None.
    pub route: Option<DexRoute>,
    /// Решение 177: продажа ВЫХОДА из позиции — монета уже куплена и продаётся (авто-продажа сделки, застрявшая монета,
    /// «Продать сейчас», «Продать и перегнать», «Продать» в «Балансах»). Ставит ядро (core/trade_ops.rs), адаптер не
    /// угадывает. DEX: расхождение локального расчёта с квотером — не отказ, а продажа по выходу квотера (ответ сети),
    /// минимум и худшая цена — от него. false — покупка, вход в сделку, циклы DEX ↔ DEX, сухая сделка: как раньше.
    #[serde(default)]
    pub exit_position: bool,
    /// Решение 247 (Тигран 10.11: согласовано): продажа DEX — пере-замер выхода при откате пробы по minOut (Remeasure).
    /// None — пере-замера нет (покупки, сухая сделка, предпроверка, «Продать» в «Балансах»): откат — ошибка, как раньше.
    /// t78 (дополнение 10.11): у первой продажи сделки — есть (граница «итог ≥ 0»; t81 — core/remeasure.rs PROFITABLE_OPEN).
    #[serde(default)]
    pub remeasure: Option<Remeasure>,
}

/// Решение 247: условия пере-замера продажи DEX (ставит ядро — core/remeasure.rs). Адаптер после отката пробы по
/// minOut симулирует ту же продажу на текущем блоке с наименьшим допустимым minOut и берёт реальный выход R; граница
/// выхода — наименьшее из R и выхода модели при налоге, наблюдённом в сети (`tax_seen_pct`; пересчёт пропорционально
/// входу — нижняя граница выхода: выход пула вогнут по входу); minOut = граница × (1 − exec.dex_slippage_pct).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Remeasure {
    /// Наименьший выход продажи (USDT, валюта котировки), при котором держится порог убытка, по которому ядро решило
    /// продать; у кнопки человека — 0 (решение 235). Граница выхода ниже — повтора нет.
    pub floor: Decimal,
    /// Налог перевода монеты, наблюдённый в сети в этой сделке, % (недостача зачисления вывода: отправлено минус
    /// комиссия вывода из ответа биржи, если она в монете, иначе вся недостача). None — не наблюдался.
    #[serde(default)]
    pub tax_seen_pct: Option<Decimal>,
    /// t81 (дополнение к 247 п. Г, Тигран 10.11: согласовано): да — продажу можно исполнить любым исполнителем DEX
    /// (агрегатор, если адаптер так настроен и его маршрут лучше, — без пере-замера; свой сборщик — с ним). Ставит ядро
    /// только на путях, где пере-замер добавил t78 (core/remeasure.rs PROFITABLE_OPEN). Нет — только свой сборщик.
    #[serde(default)]
    pub any_executor: bool,
}

/// Итоговое состояние исполнения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeStatus {
    Filled,
    PartiallyFilled,
    NotFilled,
}

/// Фактический результат покупки или продажи.
/// Решение 219: комиссия — СПИСОК частей (любое число монет: Bitget BGB + монета, Gate GT + монета, сделки ордера в
/// разных монетах). Старые записи (одна комиссия: поля fee и fee_asset) читаются — `TradeResultRaw` ниже.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "TradeResultRaw")]
pub struct TradeResult {
    pub request: TradeRequest,
    pub status: TradeStatus,
    pub filled_qty: Decimal,
    pub avg_price: Decimal,
    /// Фактическая комиссия по монетам (у DEX — газ в BNB, подробности в chain). Пусто — комиссии нет.
    pub fees: Vec<Fee>,
    /// Идентификатор ордера на бирже или хеш транзакции свопа.
    pub venue_ref: String,
    /// DEX: транзакция в сети (каналы, блок, газ). Биржа — None.
    pub chain: Option<ChainTx>,
    pub finished_at: DateTime<Utc>,
    /// Кто исполнил своп DEX (решение 121, часть B7) — текст для журнала и хронологии сделки: «свой сборщик: прямой /
    /// умный / бандл» или «KyberSwap»; решение 177 — через «; » пометка исполнения («… — продажа по квотеру»). Ядро его
    /// не разбирает, только показывает. Биржа — None.
    #[serde(default)]
    pub executor: Option<String>,
}

impl TradeResult {
    /// Что делает: сколько комиссии взято в этой монете (сумма частей с тем же тикером).
    /// Принимает: тикер. Возвращает: сумму (0 — в этой монете не брали). Ошибок нет.
    pub fn fee_in(&self, asset: &str) -> Decimal {
        self.fees
            .iter()
            .filter(|f| f.asset == asset)
            .map(|f| f.amount)
            .sum()
    }
}

/// Вид записи TradeResult при чтении (сделки в БД, JSON): новый формат — `fees`; старый (до решения 219) — одна
/// комиссия `fee` + `fee_asset`. Пишется всегда новый формат (derive Serialize у TradeResult).
#[derive(Deserialize)]
struct TradeResultRaw {
    request: TradeRequest,
    status: TradeStatus,
    filled_qty: Decimal,
    avg_price: Decimal,
    #[serde(default)]
    fees: Option<Vec<Fee>>,
    #[serde(default)]
    fee: Option<Decimal>,
    #[serde(default)]
    fee_asset: Option<String>,
    venue_ref: String,
    chain: Option<ChainTx>,
    finished_at: DateTime<Utc>,
    #[serde(default)]
    executor: Option<String>,
}

impl From<TradeResultRaw> for TradeResult {
    /// Старая запись: fee + fee_asset → список из одной части (ноль — пусто). Ошибок нет.
    fn from(r: TradeResultRaw) -> Self {
        let fees = r.fees.unwrap_or_else(|| {
            Fee::single(r.fee.unwrap_or_default(), r.fee_asset.unwrap_or_default())
        });
        TradeResult {
            request: r.request,
            status: r.status,
            filled_qty: r.filled_qty,
            avg_price: r.avg_price,
            fees,
            venue_ref: r.venue_ref,
            chain: r.chain,
            finished_at: r.finished_at,
            executor: r.executor,
        }
    }
}
