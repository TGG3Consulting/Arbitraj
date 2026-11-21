//! Файл: report/sig_model.rs
//! Что делает: решение 255 — данные книги «Сигналы и сделки»: сигнал (окно — единица деления файла), строки главного
//! листа (одна на направление окна, то есть пару «биржа 1 → биржа 2», с привязанными сделками) и строки объёмных листов
//! «Тики», «Стаканы», «Ликвидность пула» с ключом сигнала (id окна) и временем. Время — текстом UTC с миллисекундами.
//! Кто вызывает: report/sig_link.rs, sig_frames.rs, sig_episodes.rs, sig_xlsx.rs, sig_parts.rs.
//! От чего зависит: chrono, rust_decimal.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

/// Строка главного листа — колонки ровно как в задаче Тиграна 11.11 (report/sig_xlsx.rs MAIN).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MainRow {
    pub coin: String,
    pub contract: String,
    pub venue1: String,
    pub venue2: String,
    pub kind1: String,
    pub kind2: String,
    pub win_start: String,
    pub win_end: String,
    pub trade1_at: String,
    pub trade2_at: String,
    pub trade1_size: String,
    pub trade2_size: String,
    pub ticks: String,
    pub book: String,
    pub liquidity: String,
}

/// Сделка ленты биржи (лист «Тики»).
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub key: String,
    pub venue: String,
    /// Время сделки у биржи, Unix мс.
    pub ex_ms: i64,
    /// Время получения кадра ботом, Unix нс.
    pub recv_ns: i64,
    pub price: Decimal,
    pub qty: Decimal,
    pub taker: String,
    pub id: String,
}

/// Стакан на момент кадра (лист «Стаканы»): сводка всех уровней кадра, без самих уровней.
#[derive(Debug, Clone, PartialEq)]
pub struct BookRow {
    pub key: String,
    pub venue: String,
    /// Откуда: снимок N уровней потока стакана / стакан всей глубины по изменениям / запись эпизода.
    pub source: String,
    pub ex_ms: Option<i64>,
    /// Время кадра (получение ботом).
    pub at: DateTime<Utc>,
    pub bid: Option<Decimal>,
    pub ask: Option<Decimal>,
    pub levels: (usize, usize),
    pub usd: (Decimal, Decimal),
    /// Пометка (стакан по изменениям недостоверен — разрыв версий и т. п.); пусто — нет.
    pub note: String,
}

/// Запись о пуле (лист «Ликвидность пула»): точка цены DEX окна, состояние пула или глубина пула.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiqRow {
    pub key: String,
    pub at: Option<DateTime<Utc>>,
    /// Откуда: «окно: точка цены DEX», «окно: состояние пула», «эпизод: состояние пула», «эпизод: глубина пула».
    pub source: String,
    /// Направление «биржа 1 → биржа 2».
    pub direction: String,
    pub venue: String,
    pub pool: String,
    /// Вид пула и семейство (pancakeswap_v3 …).
    pub kind: String,
    pub block: Option<u64>,
    /// Сторона DEX (покупка / продажа), объём сделки или тика.
    pub side: String,
    pub price: Option<Decimal>,
    pub qty: Option<Decimal>,
    /// Глубина стороны в полосе, USDT (карточка эпизода).
    pub depth_usd: Option<Decimal>,
    /// Состояние пула (резервы, sqrtPrice, ликвидность …) текстом JSON, числа — десятичные.
    pub state: String,
}

/// Сигнал: окно (или сделка без окна) — строки главного листа и объёмных листов.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sig {
    pub key: String,
    /// Время начала (для порядка и деления по датам).
    pub at: DateTime<Utc>,
    pub rows: Vec<MainRow>,
    pub ticks: Vec<Tick>,
    pub books: Vec<BookRow>,
    pub liq: Vec<LiqRow>,
}

/// Время текстом UTC с миллисекундами. Ошибок нет.
pub fn ts(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// Время из Unix мс текстом UTC; вне диапазона — пусто. Ошибок нет.
pub fn ts_ms(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms)
        .map(ts)
        .unwrap_or_default()
}

/// Число без лишних нулей. Ошибок нет.
pub fn num(d: Decimal) -> String {
    d.normalize().to_string()
}
