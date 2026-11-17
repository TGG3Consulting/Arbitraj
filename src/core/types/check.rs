//! Файл: core/types/check.rs
//! Что делает: результаты проверки монет и пулов — налоги токена (покупка, продажа, перевод),
//! влияние крючка V4, состояние проверки (не проверено / известно / ошибка с причиной)
//! и запись чёрного списка с причиной.
//! Кто вызывает: адаптер DEX (проверяет и шлёт события), ядро (справочник, чёрный список, расчёт),
//! интерфейс (экран «Монеты»), уведомления.
//! От чего зависит: core/types/market.rs, core/types/pool.rs.
//! Настройки: пороги — tax.*, hooks.* (их применяет ядро и адаптер, не эти типы).
//!
//! Решения 30 и 32: пока проверка не закончена — торговли нет; ошибка проверки — чёрный список с причиной;
//! перепроверка по циклу; прошедшая монета выходит из списка сама; ручной чёрный список (решение 36).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::{Asset, VenueId};
use super::pool::{HookGroup, PoolRef};

/// Состояние проверки чего-либо (налога монеты, влияния крючка).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckState<T> {
    /// Ещё не проверено — торговать нельзя.
    NotChecked,
    /// Проверено, значение известно.
    Known(T),
    /// Проверка не прошла — причина как есть (ответ узла, откат пробного обмена и т.п.).
    Failed { reason: String },
}

/// Налоги токена в процентах от суммы (0–100).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenTaxes {
    /// Налог при покупке (получили меньше монет, чем отдал пул).
    pub buy_pct: Decimal,
    /// Налог при продаже (пул получил меньше монет, чем мы отправили).
    pub sell_pct: Decimal,
    /// Налог при простом переводе (кошелёк → биржа).
    pub transfer_pct: Decimal,
}

/// Вид налога — для причины чёрного списка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaxKind {
    Buy,
    Sell,
    Transfer,
}

/// Результат проверки налогов монеты на площадке DEX.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoinCheck {
    pub venue: VenueId,
    pub asset: Asset,
    pub taxes: CheckState<TokenTaxes>,
    /// Источник проверки (архитектура подключаемая, решение 30): например «eth_call» — свой пробный обмен.
    pub source: String,
    /// Когда проверено; None — ещё не проверялось.
    pub checked_at: Option<DateTime<Utc>>,
    /// Почему проверка ещё не выполнена (taxes = NotChecked): «нет пула ≥ dex.min_pool_liquidity_usd», «нет пула-источника
    /// USDT для пробы», «узел: …», «не задана настройка …». None — не ждёт или причина не известна.
    #[serde(default)]
    pub wait: Option<String>,
}

/// Решение 248 (техлид 10.11 по поручению Тиграна, кейс SMARS): продажа выхода DEX откатилась «мало на выходе», и
/// адаптер замерил реальный выход той же продажи симуляцией с наименьшим минимумом (1 ед.) с реального баланса
/// кошелька. Ядро по нему закрывает монету для новых входов (core/exit_gap.rs), если замер ниже модели больше допуска.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExitGap {
    pub venue: VenueId,
    pub asset: Asset,
    /// Сделка, в которой замерено.
    pub deal_id: String,
    /// Выход по модели (сумма ожидаемых долей при модельном налоге), USDT.
    pub model_usdt: Decimal,
    /// Граница выхода по замеру (наименьшее из симуляции и выхода при налоге, наблюдённом в сети), USDT.
    pub real_usdt: Decimal,
    /// Модельный налог монеты на шаге продажи (наибольший по долям), %.
    pub model_tax_pct: Decimal,
}

/// Влияние крючка на обмен, измеренное пробным обменом (решение 32), в процентах от суммы.
/// Положительное — получаем меньше, чем по математике пула без крючка.
/// Решение «335 (лесенка влияния)»: влияние — функция суммы и стороны. buy / sell — лесенка точек стороны (по
/// возрастанию входа в пул), buy_pct / sell_pct — наибольшее влияние по точкам стороны (верхняя оценка для мест, где
/// сумма неизвестна). Пустая лесенка — влияние одно на все суммы (buy_pct / sell_pct), как до решения.
/// Ревью 3 (Б4, Н-в): сторона может «не пройти» отдельно — buy_fail / sell_fail (причина): квотер отказал уже на
/// наименьшей сумме стороны; другая сторона работает. refused — отказы квотера по точкам последнего замера (сторона —
/// покупка ли, вход, причина): точка и большие суммы стороны не измерены; не хранится (только итог замера для пробы
/// перед сделкой — exec/trade_probe.rs).
/// Ревью 4 лесенки (Н1): лесенки сторон — общие (HookLadder = Arc): копия итогов пула (check/state.rs pool) и допуск
/// котировки (quote/select.rs) берут ссылку на ту же лесенку, а не копию точек.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HookImpact {
    pub buy_pct: Decimal,
    pub sell_pct: Decimal,
    #[serde(default, with = "ladder_serde")]
    pub buy: HookLadder,
    #[serde(default, with = "ladder_serde")]
    pub sell: HookLadder,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buy_fail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sell_fail: Option<String>,
    #[serde(skip)]
    pub refused: Vec<(bool, alloy_primitives::U256, String)>,
}

/// Лесенка стороны (точки по возрастанию входа), общая для копий итога (ревью 4 лесенки, Н1).
pub type HookLadder = std::sync::Arc<Vec<HookStep>>;

/// Сериализация HookLadder как обычного списка точек (формат JSON для экрана тот же, что у Vec). Ошибок сверх serde нет.
mod ladder_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{HookLadder, HookStep};

    pub fn serialize<S: Serializer>(v: &HookLadder, s: S) -> Result<S::Ok, S::Error> {
        v.as_slice().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<HookLadder, D::Error> {
        Vec::<HookStep>::deserialize(d).map(std::sync::Arc::new)
    }
}

/// Точка лесенки влияния крючка (решение «335 (лесенка влияния)»): вход в пул стороны в минимальных единицах токена
/// входа (покупка — встречный токен пула: USDT или промежуточная монета; продажа — монета), сумма в USDT (0 — не
/// считалась: точка пробы перед сделкой) и влияние на этой сумме, %.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookStep {
    pub amount_in: alloy_primitives::U256,
    pub usd: Decimal,
    pub pct: Decimal,
    /// Ревью 4 лесенки (Н3): вход точки пробы перед сделкой в токене входа стороны (decimals токена; экран «Монеты»
    /// показывает сумму, а не hex минимальных единиц); None — точка цикла (у неё есть usd) или decimals токена не было
    /// в памяти.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coins: Option<Decimal>,
    /// Выход квотера на этой сумме, минимальные единицы (ревью Н7: проба перед сделкой отдаёт его сверке прямого
    /// режима, второй вызов квотера на той же сумме не нужен); None — не сохранялся (точка лесенки после выравнивания).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quoter_out: Option<alloy_primitives::U256>,
}

/// Результат проверки пула с крючком группы B или C.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolCheck {
    pub venue: VenueId,
    /// Вид пула у адаптера (id области «DEX»).
    pub family: String,
    pub pool: PoolRef,
    /// Монета пула (вторая валюта — USDT).
    pub asset: Asset,
    pub group: HookGroup,
    pub impact: CheckState<HookImpact>,
    pub checked_at: Option<DateTime<Utc>>,
}

/// Почему монета в чёрном списке.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BlockReason {
    /// Проверка налогов ещё не закончена.
    NotChecked,
    /// Проверка не прошла — причина от адаптера.
    CheckFailed { reason: String },
    /// Налог выше порога настройки tax.max_*_pct.
    TaxAboveLimit {
        tax: TaxKind,
        pct: Decimal,
        limit: Decimal,
    },
    /// Ручной чёрный список из интерфейса (настройки coin.blocked, coin.blocked_note).
    Manual { note: Option<String> },
}

/// Запись чёрного списка монеты.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoinBlock {
    pub asset: Asset,
    pub reason: BlockReason,
    pub since: DateTime<Utc>,
}
