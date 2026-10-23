//! Файл: venues/bsc_dex/pools/bins.rs
//! Что делает: локальные резервы бинов пула Infinity Bin — (reserveX, reserveY) по id непустых бинов
//! в прочитанной полосе вокруг активного бина. Нужны математике обмена Bin (math/bin_*, шаг 1c).
//! Кто вызывает: pools/load_bin.rs (заполняет), pools/events_bin.rs (перечитывает затронутые бины), math/*.
//! От чего зависит: ничего.
//! Настройки: dex.bin_depth_usd (глубина полосы — решает загрузчик).
//!
//! Бин: X = currency0, Y = currency1; getBin → (binReserveX, binReserveY, …) —
//! https://github.com/pancakeswap/infinity-core/blob/3c5ef1be1f71442d8bc60cc6fd11f9c677b714df/src/pool-bin/interfaces/IBinPoolManager.sol#L140-L149 .

use std::collections::BTreeMap;

/// Резервы бинов пула.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinMap {
    /// binStep пула (1 = 0,01%).
    pub step: u32,
    /// id → (reserveX, reserveY); только непустые бины.
    pub reserves: BTreeMap<u32, (u128, u128)>,
    /// Прочитанная полоса id (включительно): за её пределами бины неизвестны.
    pub range: (u32, u32),
}

impl BinMap {
    /// Лежит ли бин в прочитанной полосе. Ошибок нет.
    pub fn covers(&self, id: u32) -> bool {
        id >= self.range.0 && id <= self.range.1
    }

    /// Записывает резервы бина (после getBin); пустой бин удаляется. Бин вне полосы не записывается.
    /// Принимает: id, резервы X и Y. Ошибок нет.
    pub fn set(&mut self, id: u32, x: u128, y: u128) {
        if !self.covers(id) {
            return;
        }
        if x == 0 && y == 0 {
            self.reserves.remove(&id);
        } else {
            self.reserves.insert(id, (x, y));
        }
    }
}
