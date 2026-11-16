//! Файл: venues/bsc_dex/pools/depth_fail.rs
//! Что делает: память НЕУДАЧ расчёта глубины пула (решение 301, часть б; задача t132): глубина, которую по состоянию
//! пула посчитать нельзя («расчёт глубины PMM невозможен», «карта тиков не прочитана» и т. п. — ошибка
//! quote/depth_band.rs band_units), запоминается с причиной по тому же ключу, что удачная (depth_key.rs: поколение
//! состояния пула — решение 301 е2, pools/state_gen.rs; настройки глубины; курс встречного токена). Пока ключ тот же,
//! расчёт не повторяется — книга отдаёт ту же причину. (До е2 к ключу добавлялось время получения состояния пула
//! Pool.received_at — страховка на смену состояния без смены блока/logIndex; поколение её покрывает.) Раньше неудача не запоминалась (ошибка выходила до записи в книгу), и
//! каждое построение графа маршрутов (pools/graph.rs build — на каждую версию реестра) заново считало глубину таких
//! пулов; у DODO PMM это до 192 удвоений + 64 шага двоичного поиска по обменам (quote/pmm_depth.rs).
//! Неудача перевода глубины в USDT (нет USDT, нет курса встречного токена) здесь НЕ запоминается: она зависит не только
//! от состояния пула (USDT площадки — настройка), её считает depth_book, как раньше.
//! Кто вызывает: pools/depth_book.rs.
//! От чего зависит: depth_key.rs, core/types.rs (PoolRef).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::core::types::PoolRef;

use super::depth_key::Key;

/// Неудача: ключ глубины, причина.
struct Failed {
    key: Key,
    why: String,
}

/// Неудачи расчёта глубины по пулам. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Fails {
    m: Arc<Mutex<HashMap<PoolRef, Failed>>>,
}

impl Fails {
    /// Причина запомненной неудачи пула, если ключ тот же. Ошибок нет.
    pub fn get(&self, r: &PoolRef, key: &Key) -> Option<String> {
        self.lock()
            .get(r)
            .filter(|f| f.key == *key)
            .map(|f| f.why.clone())
    }

    /// Запомнить неудачу пула (заменяет прежнюю). Ошибок нет.
    pub fn put(&self, r: &PoolRef, key: Key, why: String) {
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
