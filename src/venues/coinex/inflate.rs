//! Файл: venues/coinex/inflate.rs
//! Что делает: распаковка кадров WebSocket CoinEx в текст. Один распаковщик на соединение с переиспользуемым буфером
//! (урок 13, решения 130–131: без новых выделений памяти на каждый кадр). Вид сжатия определяется по заголовку самих
//! данных — gzip (RFC 1952: первые байты 1f 8b), zlib (RFC 1950: CM = 8 и (CMF·256 + FLG) кратно 31), иначе — сырой
//! deflate (RFC 1951); какой пришёл, ws_conn.rs один раз пишет в журнал.
//! Кто вызывает: ws_conn.rs (каждый двоичный кадр), тесты.
//! От чего зависит: flate2.
//! Настройки: нет (предел размера распакованного кадра передаёт ws.rs — http.max_body_bytes).
//!
//! Официально (Integration Guide, https://docs.coinex.com/api/v2/guide): «The response from the WS server is compressed
//! using zip and must be decompressed first» — какой именно формат («zip»), не сказано: проверить на живой записи
//! (tests/README.md, раздел CoinEx).

use std::io::Read;

use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};

/// Вид сжатия кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Gzip,
    Zlib,
    Deflate,
}

impl Format {
    /// Вид сжатия по первым байтам данных (см. шапку). Ошибок нет.
    pub fn of(data: &[u8]) -> Format {
        match data {
            [0x1f, 0x8b, ..] => Format::Gzip,
            [cmf, flg, ..]
                if cmf & 0x0f == 8 && (u16::from(*cmf) * 256 + u16::from(*flg)) % 31 == 0 =>
            {
                Format::Zlib
            }
            _ => Format::Deflate,
        }
    }

    /// Название для журнала. Ошибок нет.
    pub fn name(self) -> &'static str {
        match self {
            Format::Gzip => "gzip",
            Format::Zlib => "zlib",
            Format::Deflate => "deflate",
        }
    }
}

/// Распаковщик соединения: буфер живёт всё соединение.
#[derive(Default)]
pub struct Inflater {
    buf: Vec<u8>,
}

impl Inflater {
    /// Что делает: распаковывает кадр в текст UTF-8 (в своём буфере, без копии наружу).
    /// Принимает: сжатые данные, предел размера распакованного кадра (байт).
    /// Возвращает: (текст, вид сжатия). Ошибки: текст причины — данные не распаковались, больше предела, не UTF-8.
    pub fn text(&mut self, data: &[u8], max: usize) -> Result<(&str, Format), String> {
        let f = Format::of(data);
        self.buf.clear();
        let limit = max as u64 + 1;
        let read = match f {
            Format::Gzip => GzDecoder::new(data).take(limit).read_to_end(&mut self.buf),
            Format::Zlib => ZlibDecoder::new(data)
                .take(limit)
                .read_to_end(&mut self.buf),
            Format::Deflate => DeflateDecoder::new(data)
                .take(limit)
                .read_to_end(&mut self.buf),
        };
        read.map_err(|e| format!("кадр не распакован ({}): {e}", f.name()))?;
        if self.buf.len() > max {
            return Err(format!(
                "распакованный кадр больше {max} байт (http.max_body_bytes)"
            ));
        }
        let text = std::str::from_utf8(&self.buf)
            .map_err(|_| format!("распакованный кадр ({}) не UTF-8", f.name()))?;
        Ok((text, f))
    }
}
