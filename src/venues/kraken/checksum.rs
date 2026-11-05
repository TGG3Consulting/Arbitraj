//! Файл: venues/kraken/checksum.rs
//! Что делает: чек-сумма стакана Kraken WebSocket v2 — CRC32 строки из верхних 10 asks (по возрастанию) и 10 bids (по
//! убыванию): у каждого уровня цена и количество в формате пары без точки и без ведущих нулей.
//! Кто вызывает: local_book.rs, тесты.
//! От чего зависит: core/types (PriceLevel), rust_decimal.
//! Настройки: нет.
//!
//! Официально — https://docs.kraken.com/exchange/guides/websockets/book-checksum-v2.md: «Use the top 10 price levels on
//! each side»; «Remove the decimal point. Remove leading zeros. Append the price string followed by the quantity
//! string» (пример: ask 45285.2 / 0.00100000 → «452852» + «100000»); «Concatenate asks followed by bids, and pass the
//! result to a CRC32 function. Cast the result to an unsigned 32-bit integer». Количество в примере — с 8 знаками
//! (0.00100000), а WebSocket шлёт число без хвостовых нулей — поэтому формат по знакам пары: цена — pair_decimals,
//! количество — lot_decimals из AssetPairs (у XBT/USD 1 и 8 — как в примере; сверить на живой записи). Вариант CRC32 в
//! гиде не назван — берётся стандартный (IEEE 802.3, как zlib.crc32 / binascii.crc32 Python): проверка — на записи.

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

/// Уровней каждой стороны в чек-сумме — «top 10 price levels on each side».
pub const LEVELS: usize = 10;

/// Что делает: чек-сумма. Принимает: asks по возрастанию, bids по убыванию (не больше LEVELS — лишние не берутся),
/// знаки цены и количества пары. Возвращает: CRC32. Ошибок нет.
pub fn of(asks: &[PriceLevel], bids: &[PriceLevel], price_dp: u32, lot_dp: u32) -> u32 {
    let mut s = String::with_capacity(2 * LEVELS * 24);
    for l in asks.iter().take(LEVELS).chain(bids.iter().take(LEVELS)) {
        s.push_str(&field(l.price, price_dp));
        s.push_str(&field(l.qty, lot_dp));
    }
    crc32(s.as_bytes())
}

/// Число в формате с dp знаками, без точки и ведущих нулей («0.00100000» → «100000»). Ошибок нет.
pub fn field(v: Decimal, dp: u32) -> String {
    let t = format!("{:.*}", dp as usize, v.round_dp(dp));
    let digits: String = t.chars().filter(|c| *c != '.').collect();
    let trimmed = digits.trim_start_matches('0');
    trimmed.to_string()
}

/// CRC32 IEEE 802.3 (полином 0xEDB88320 в отражённом виде, начальное и конечное XOR 0xFFFFFFFF). Ошибок нет.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}
