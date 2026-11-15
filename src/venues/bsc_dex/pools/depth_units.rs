//! Файл: venues/bsc_dex/pools/depth_units.rs
//! Что делает: решение 307 (задача t137) — кривая пула для глубины (quote/depth_band.rs band_units: сколько встречного
//! токена до границы полосы в каждую сторону) считается по записи пула и полосе и помнится по (поколение состояния,
//! полоса) — UnitsKey (depth_fail.rs). Курс встречного токена к USDT в неё не входит: при смене курса (пул X–USDT
//! сменил состояние) книга глубины берёт кривую из памяти и пересчитывает только перевод в USDT. Раньше кривая
//! пересчитывалась у всех пулов с этим встречным токеном (тысячи пулов с WBNB) — в т. ч. у DODO PMM до 192 удвоений и
//! 64 шагов двоичного поиска (quote/pmm_depth.rs). Неудача кривой — память неудач по тому же UnitsKey (решение 301 б).
//! Бит в бит: band_units — чистая функция записи и полосы (состояние, тики, бины входят в поколение — pools/state_gen.rs).
//! Кто вызывает: pools/depth_book.rs (depth_prep_r).
//! От чего зависит: depth_book.rs (Book: fails, calcs), depth_fail.rs, quote/depth_band.rs, registry.rs.
//! Настройки: нет (полосу dex.depth_band_pct даёт вызывающий в ключе).

use std::sync::atomic::Ordering;

use alloy_primitives::U256;

use super::super::quote::depth_band;
use super::depth_book::Book;
use super::depth_fail::UnitsKey;
use super::registry::Entry;

/// Кривая пула: (покупка — отдано в пул, продажа — получено из пула, полоса прочитана целиком), минимальные единицы
/// встречного токена.
pub type Units = (U256, U256, bool);

impl Book {
    /// Кривая пула заново (памяти на `key` нет): неудача на том же ключе — та же причина без расчёта; иначе расчёт
    /// (счётчик calcs, признак `recomputed`), неудача запоминается, удача снимает прежнюю неудачу.
    /// Принимает: запись, ключ (поколение, полоса), признак «считалось заново». Возвращает: кривую.
    /// Ошибки (текст): полоса вне 0–100%, расчёт по состоянию пула невозможен (depth_band.rs band_units).
    pub(super) fn units(&self, e: &Entry, key: UnitsKey, recomputed: &mut bool) -> Result<Units, String> {
        let r = &e.pool.reference;
        if let Some(why) = self.fails.get(r, &key) {
            return Err(why);
        }
        self.calcs.fetch_add(1, Ordering::Relaxed);
        *recomputed = true;
        match depth_band::band_units(e, key.band) {
            Ok(u) => {
                self.fails.clear(r);
                Ok(u)
            }
            Err(why) => {
                self.fails.put(r, key, why.clone());
                Err(why)
            }
        }
    }
}
