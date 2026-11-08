//! Файл: core/types/delist.rs
//! Что делает: делистинги площадки (решение 175, Тигран 05.11.2026) — какие пары биржа снимает с торгов и когда.
//! Живой случай 05.11: KuCoin снял ION/USDT с торгов, а бот купил ION на KuCoin — «разница цены» была из-за
//! делистинга. Ядро по этому списку не торгует монету, если на любой стороне сделки она в делистинге и до него
//! меньше delist.block_hours (core/delist_check.rs).
//! Как у удержаний вывода (решение 166, hold.rs): у площадки может не быть официального способа это узнать — тогда
//! ответ «неизвестно» (unknown), а НЕ «делистингов нет»: ядро такую площадку не блокирует, а пишет в журнал.
//! Решение 226: у биржи может быть несколько источников (справочник пар + анонсы делистинга); сбой второстепенного
//! источника не отменяет основной — он идёт в warnings (видно на экране, журнал core.delist один раз).
//! Кто вызывает: gateway/contract.rs (delistings), адаптеры бирж (venues/*/delist.rs), core/delist.rs,
//! core/delist_check.rs, api (GET /api/delistings).
//! От чего зависит: chrono, serde.
//! Настройки: нет (delist.st_block читает core/delist_check.rs и передаёт в find).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Одна пара в делистинге.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelistItem {
    /// Тикер монеты так, как его пишет площадка в справочнике сетей и пар (базовая монета пары).
    pub ticker: String,
    /// Символ пары у площадки (например «ION_USDT», «IONUSDT»).
    pub symbol: String,
    /// Когда прекращаются торги. None — площадка сообщила, что пара в делистинге, но не сообщила когда.
    pub trading_end: Option<DateTime<Utc>>,
    /// Когда закрывается вывод монеты, если площадка это сообщает. None — не сообщает.
    pub withdraw_end: Option<DateTime<Utc>>,
    /// Откуда известно — эндпоинт и поле API (для человека), например «GET /spot/currency_pairs: delisting_time».
    pub source: String,
    /// Решение 226 (уточнение 08.11): запись — метка риска ST (MEXC st, KuCoin st, Bybit stTag), а не делистинг.
    /// Блокирует, только если у площадки delist.st_block = да (тогда — как делистинг без времени); иначе монета
    /// торгуется (Тигран: «на супер волатильных монетах и появляются нормальные спреды»).
    #[serde(default)]
    pub st: bool,
}

/// Делистинги площадки сейчас. Пусто и unknown = None — площадка по своему API сообщила, что делистингов нет.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delistings {
    pub items: Vec<DelistItem>,
    /// Some(почему) — у площадки нет официального способа узнать делистинги (или он не описан в её документации);
    /// items тогда пуст. None — ответ по фактам API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown: Option<String>,
    /// Решение 226: второстепенный источник не прочитан (например, анонсы делистинга: настройка не задана, сбой
    /// запроса) — пары из основного источника при этом действуют. Пусто — все источники прочитаны.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl Delistings {
    /// Ответ по фактам API: список пар в делистинге (пустой — делистингов нет). Ошибок нет.
    pub fn of(items: Vec<DelistItem>) -> Self {
        Self {
            items,
            unknown: None,
            warnings: vec![],
        }
    }

    /// «Неизвестно» с причиной (нет в документации площадки). Ошибок нет.
    pub fn unknown(why: impl Into<String>) -> Self {
        Self {
            items: vec![],
            unknown: Some(why.into()),
            warnings: vec![],
        }
    }

    /// Площадка не может сказать, что у неё в делистинге. Ошибок нет.
    pub fn is_unknown(&self) -> bool {
        self.unknown.is_some()
    }

    /// Запись монеты по тикеру площадки. Принимает: тикер, учитывать ли метки ST (delist.st_block площадки).
    /// None — монеты в делистинге нет (или есть только метка ST, а st_block = нет). Несколько записей одной монеты
    /// (разные источники, решение 226) — строже та, где время не сообщено (такая блокирует всегда), иначе — самое
    /// раннее прекращение торгов. Ошибок нет.
    pub fn find(&self, ticker: &str, st_block: bool) -> Option<&DelistItem> {
        self.items
            .iter()
            .filter(|i| i.ticker == ticker && (st_block || !i.st))
            .min_by_key(|i| i.trading_end.map(|t| (1, t.timestamp())).unwrap_or((0, 0)))
    }
}
