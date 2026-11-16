//! Файл: venues/bsc_dex/pools/bins.rs
//! Что делает: локальные резервы бинов пула Infinity Bin — (reserveX, reserveY) по id непустых бинов
//! в прочитанной полосе вокруг активного бина. Нужны математике обмена Bin (math/bin_*, шаг 1c).
//! Кто вызывает: pools/load_bin.rs (заполняет), pools/events_bin.rs (перечитывает затронутые бины), math/*;
//! pools/load_lb_bins.rs (Liquidity Book V2.1 — та же карта), pools/off_map_bins.rs (края полосы, решение 332).
//! Ревью 332 (Н6): ответы getNextNonEmptyBin «дальше бинов нет» (NONE_BELOW / NONE_ABOVE) — одни константы на оба вида
//! пулов из бинов; края полосы «сторона прочитана до конца пула» — down_to_end / up_to_end.
//! От чего зависит: ничего.
//! Настройки: dex.bin_depth_usd (глубина полосы — решает загрузчик).
//!
//! Бин: X = currency0, Y = currency1; getBin → (binReserveX, binReserveY, …) —
//! https://github.com/pancakeswap/infinity-core/blob/3c5ef1be1f71442d8bc60cc6fd11f9c677b714df/src/pool-bin/interfaces/IBinPoolManager.sol#L140-L149 .

use std::collections::BTreeMap;
use std::sync::Arc;

/// Ответ getNextNonEmptyBin(swapForY = true) «ниже бинов нет»: type(uint24).max (TreeMath.findFirstRight —
/// infinity-core 3c5ef1be TreeMath.sol, lfj-gg/joe-v2@v2.1.0 TreeMath.sol L147). Им же загрузчики отмечают верхний край
/// полосы «вверх прочитано до конца пула» (range.1).
pub const NONE_BELOW: u32 = 0xff_ffff;
/// Ответ getNextNonEmptyBin(swapForY = false) «выше бинов нет»: 0 (TreeMath.findFirstLeft, TreeMath.sol L202). Им же
/// загрузчики отмечают нижний край полосы «вниз прочитано до конца пула» (range.0).
pub const NONE_ABOVE: u32 = 0;

/// Резервы бинов пула.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinMap {
    /// binStep пула (1 = 0,01%).
    pub step: u32,
    /// id → (reserveX, reserveY); только непустые бины.
    /// За `Arc` (решение 129): копия записи реестра не копирует бины; изменение — копия при записи, если карту держит снимок.
    pub reserves: Arc<BTreeMap<u32, (u128, u128)>>,
    /// Прочитанная полоса id (включительно): за её пределами бины неизвестны.
    pub range: (u32, u32),
}

impl BinMap {
    /// Вниз прочитано до конца пула (загрузчик дошёл до «ниже бинов нет» и поставил край NONE_ABOVE). Ошибок нет.
    pub fn down_to_end(&self) -> bool {
        self.range.0 == NONE_ABOVE
    }

    /// Вверх прочитано до конца пула (загрузчик дошёл до «выше бинов нет» и поставил край NONE_BELOW). Ошибок нет.
    pub fn up_to_end(&self) -> bool {
        self.range.1 == NONE_BELOW
    }

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
        let reserves = Arc::make_mut(&mut self.reserves);
        if x == 0 && y == 0 {
            reserves.remove(&id);
        } else {
            reserves.insert(id, (x, y));
        }
    }
}
