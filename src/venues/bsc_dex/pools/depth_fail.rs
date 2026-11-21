//! Файл: venues/bsc_dex/pools/depth_fail.rs
//! Что делает: память НЕУДАЧ расчёта глубины пула (решение 301, часть б; задача t132): глубина, которую по состоянию
//! пула посчитать нельзя («расчёт глубины PMM невозможен», «карта тиков не прочитана» и т. п. — ошибка
//! quote/depth_band.rs band_units), запоминается с причиной по тому же ключу, что удачная (depth_key.rs: поколение
//! состояния пула — решение 301 е2, pools/state_gen.rs; настройки глубины; курс встречного токена). Пока ключ тот же,
//! расчёт не повторяется — книга отдаёт ту же причину. (До е2 к ключу добавлялось время получения состояния пула
//! Pool.received_at — страховка на смену состояния без смены блока/logIndex; поколение её покрывает.) Раньше неудача не запоминалась (ошибка выходила до записи в книгу), и
//! каждое построение графа маршрутов (pools/graph.rs build — на каждую версию реестра) заново считало глубину таких
//! пулов; у DODO PMM это до 192 удвоений + 64 шага двоичного поиска по обменам (quote/pmm_depth.rs).
//! Решение 307: ключ неудачи — (поколение состояния пула, полоса) — UnitsKey: band_units зависит только от записи пула и
//! полосы, не от курса встречного токена и не от порога/гистерезиса; смена курса неудачу не сбрасывает (та же причина).
//! Тем же ключом книга помнит удачные band_units (depth_book.rs Slot.units).
//! Неудача перевода глубины в USDT (нет USDT, нет курса встречного токена) здесь НЕ запоминается: она зависит не только
//! от состояния пула (USDT площадки — настройка), её считает depth_book, как раньше.
//! Кто вызывает: pools/depth_book.rs.
//! От чего зависит: core/types.rs (PoolRef).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rust_decimal::Decimal;

use crate::core::types::PoolRef;

/// Решение 307: на чём посчитаны band_units пула — поколение состояния (решение 301 е2) и полоса dex.depth_band_pct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitsKey {
    pub state_gen: u64,
    pub band: Decimal,
}

impl UnitsKey {
    /// Годится ли память на `self` запросу `o`: поколение 0 (запись вне реестра) не годится никогда. Ошибок нет.
    pub fn same(&self, o: &UnitsKey) -> bool {
        self.state_gen != 0 && self == o
    }
}

/// Неудача: ключ band_units, причина.
struct Failed {
    key: UnitsKey,
    why: String,
}

/// Неудачи расчёта глубины по пулам. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Fails {
    m: Arc<Mutex<HashMap<PoolRef, Failed>>>,
}

impl Fails {
    /// Причина запомненной неудачи пула, если ключ тот же. Ошибок нет.
    pub fn get(&self, r: &PoolRef, key: &UnitsKey) -> Option<String> {
        self.lock()
            .get(r)
            .filter(|f| f.key.same(key))
            .map(|f| f.why.clone())
    }

    /// Запомнить неудачу пула (заменяет прежнюю). Ошибок нет.
    pub fn put(&self, r: &PoolRef, key: UnitsKey, why: String) {
        self.lock().insert(r.clone(), Failed { key, why });
    }

    /// Глубина пула посчиталась — неудача больше не нужна. Ошибок нет.
    pub fn clear(&self, r: &PoolRef) {
        self.lock().remove(r);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PoolRef, Failed>> {
        self.m.lock().unwrap_or_else(|p| p.into_inner())
    }
}
