//! Файл: core/types/directory.rs
//! Что делает: типы справочника пулов от индексатора (решение 58): какие DEX знает индексатор, какие пулы он отдал
//! по монете и что с каждым пулом сделал адаптер DEX (взят, DEX не подключена, проверка на узле не прошла …).
//! Индексатор — только справочник адресов: цены и состояние пулов — всегда со своего узла.
//! Кто вызывает: адаптер DEX (заполняет), api (экран «Монеты», таблица DEX на «Площадках»), интерфейс.
//! От чего зависит: core/types/market.rs, alloy-primitives, rust_decimal, chrono.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

use alloy_primitives::Address;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// DEX, как её называет индексатор (id — ключ таблицы сопоставления «DEX индексатора → вид пула»).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexerDex {
    pub id: String,
    pub name: String,
}

/// Что адаптер сделал с пулом из справочника.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectoryStatus {
    /// Пул ещё не проверен на своём узле.
    Unchecked,
    /// Пул проверен на своём узле (фабрика/менеджер, токены) и слушается.
    Taken,
    /// DEX индексатора не подключена: нет в таблице сопоставления, «не поддерживается», выключена или не хватает
    /// адресов. Причина — текстом («DEX X не подключена: …»).
    NotConnected(String),
    /// Проверка на своём узле не прошла (чужая фабрика, другие токены, ключ пула не найден) — причина текстом.
    Rejected(String),
    /// Пул такого вида адаптер не поддерживает (например валюта — нативный BNB, адрес 0x0) — причина текстом.
    Unsupported(String),
}

/// Пул из справочника индексатора с итогом адаптера.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryPool {
    /// Монета, по которой спрашивали индексатор (адрес — из API биржи).
    pub token: Address,
    /// id DEX у индексатора.
    pub dex_id: String,
    /// Адрес пула (V2/V3) или PoolId (V4/Infinity) — как отдал индексатор.
    pub pool: String,
    pub base: Address,
    pub quote: Address,
    /// Тикеры токенов пула — как у индексатора (только для показа).
    pub base_symbol: String,
    pub quote_symbol: String,
    /// Ликвидность пула в USD по данным индексатора — только для показа: ни в отборе, ни в расчёте не участвует
    /// (решение 58; у мусорных пулов бывает поддельной). None — нет в ответе.
    pub reserve_usd: Option<Decimal>,
    pub status: DirectoryStatus,
}

/// Монета справочника: когда спрашивали индексатор, сколько страниц прочитано и последняя ошибка.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryToken {
    pub token: Address,
    pub ticker: String,
    /// Когда получен последний полный ответ. None — ещё ни разу.
    pub fetched_at: Option<DateTime<Utc>>,
    pub pages: u32,
    /// Сколько пулов отдал индексатор по монете (все пары, не только с USDT и промежуточными).
    pub total: usize,
    /// Последняя ошибка запроса; None — последний запрос удался.
    pub error: Option<String>,
}

/// Справочник пулов целиком — для интерфейса.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PoolDirectory {
    /// Адрес API индексатора без ключа (index.api_url) и id сети (index.network); None — не заданы.
    pub source: Option<String>,
    pub network: Option<String>,
    /// Подпись источника, если её требуют условия индексатора (index.attribution_text / _url).
    pub attribution_text: Option<String>,
    pub attribution_url: Option<String>,
    /// DEX сети у индексатора и когда список получен.
    pub dexes: Vec<IndexerDex>,
    pub dexes_at: Option<DateTime<Utc>>,
    pub tokens: Vec<DirectoryToken>,
    /// Пулы монет с USDT или промежуточной монетой, а также с нативным BNB (статус «не поддержан»); остальные пары
    /// не нужны и не показываются.
    pub pools: Vec<DirectoryPool>,
    /// Почему справочник не обновляется (не задана настройка, нет связи); None — обновляется.
    pub error: Option<String>,
}
