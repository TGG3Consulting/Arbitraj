//! Файл: venues/bsc_dex/pools/ticks.rs
//! Что делает: локальная карта тиков пула V3/CL — liquidityGross и liquidityNet по инициализированным тикам
//! в прочитанных словах битмапа вокруг текущего тика. Нужна математике обмена (math/*, шаг 1c).
//! Обновляется событиями Mint/Burn (V3) и ModifyLiquidity (CL) только внутри прочитанных слов:
//! за их пределами прежнее состояние неизвестно, и математика туда не заходит.
//! Кто вызывает: pools/load_v3.rs, pools/load_cl.rs (заполняют), pools/events_v3.rs, events_cl.rs (обновляют), math/*.
//! От чего зависит: ничего.
//! Настройки: dex.tick_words (сколько слов читается — решает загрузчик).
//!
//! Слово битмапа: compressed = floor(tick / tickSpacing); word = compressed >> 8 (256 тиков на слово) —
//! TickBitmap.position: https://github.com/Uniswap/v3-core/blob/5ea68a77cb541c6b3812df783be71ba8c081e25f/contracts/libraries/TickBitmap.sol#L14-L17 ;
//! обновление тиков позицией: Tick.update (liquidityGross += |Δ|, liquidityNet += Δ у нижнего, −Δ у верхнего) —
//! https://github.com/Uniswap/v3-core/blob/5ea68a77cb541c6b3812df783be71ba8c081e25f/contracts/libraries/Tick.sol#L110-L140 .
//! Тик снимается, когда liquidityGross = 0 (Tick.update: flipped).

use std::collections::BTreeMap;

/// Данные одного инициализированного тика.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickData {
    pub gross: u128,
    pub net: i128,
}

/// Карта тиков пула.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickMap {
    pub spacing: i32,
    pub ticks: BTreeMap<i32, TickData>,
    /// Прочитанные слова битмапа (включительно): (первое, последнее).
    pub words: (i16, i16),
}

impl TickMap {
    /// Слово битмапа, в котором лежит тик. Ошибок нет.
    pub fn word_of(tick: i32, spacing: i32) -> i16 {
        let compressed = tick.div_euclid(spacing);
        (compressed >> 8) as i16
    }

    /// Лежит ли тик в прочитанных словах. Ошибок нет.
    pub fn covers(&self, tick: i32) -> bool {
        let w = Self::word_of(tick, self.spacing);
        w >= self.words.0 && w <= self.words.1
    }

    /// Изменение ликвидности позиции [lower, upper) на Δ (Mint: +amount, Burn: −amount, ModifyLiquidity: Δ).
    /// Тики вне прочитанных слов не трогаются. Принимает: границы и Δ. Возвращает: false — Δ не сходится
    /// с картой (gross ушёл бы ниже нуля): карта недостоверна, пул нужно перечитать.
    pub fn modify(&mut self, lower: i32, upper: i32, delta: i128) -> bool {
        self.update(lower, delta, false) && self.update(upper, delta, true)
    }

    /// Обновление одного тика (как Tick.update). Возвращает false при расхождении.
    fn update(&mut self, tick: i32, delta: i128, upper: bool) -> bool {
        if !self.covers(tick) {
            return true;
        }
        let cur = self
            .ticks
            .get(&tick)
            .copied()
            .unwrap_or(TickData { gross: 0, net: 0 });
        let gross = if delta >= 0 {
            cur.gross.checked_add(delta.unsigned_abs())
        } else {
            cur.gross.checked_sub(delta.unsigned_abs())
        };
        let Some(gross) = gross else { return false };
        let net = if upper {
            cur.net.checked_sub(delta)
        } else {
            cur.net.checked_add(delta)
        };
        let Some(net) = net else { return false };
        if gross == 0 {
            self.ticks.remove(&tick);
        } else {
            self.ticks.insert(tick, TickData { gross, net });
        }
        true
    }
}
