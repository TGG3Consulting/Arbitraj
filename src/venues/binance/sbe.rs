//! Файл: venues/binance/sbe.rs
//! Что делает: решение 425 — разбор двоичного кадра SBE рынка Binance (stream-sbe.binance.com): заголовок сообщения и
//! только одно нужное сообщение — DepthDiffStreamEvent (поток изменений стакана <symbol>@depth, 20 мс). Остальные
//! сообщения схемы (сделки, лучшая цена, частичный стакан) не разбираются — отдаётся их номер шаблона.
//! Кто вызывает: sbe_conn.rs (кадр WebSocket Binary), тесты.
//! От чего зависит: core/types (PriceLevel), rust_decimal.
//! Настройки: нет.
//!
//! Официально: схема https://github.com/binance/binance-spot-api-docs/blob/master/sbe/schemas/stream_1_0.xml
//! (package="spot_stream", id="1", version="0", semanticVersion="5.2", byteOrder="littleEndian"; сверено по коммиту
//! репозитория 473c101d от 2026-11-18); sbe-market-data-streams.md «Diff. Depth Streams»: «SBE Message Name:
//! DepthDiffStreamEvent», «Update Speed: 20ms»; «All time and timestamp fields are in microseconds»; «You will receive
//! market data events in SBE in WebSocket binary frames». Разбор — по FIX SBE (faqs/sbe_faq.md ссылается на
//! https://www.fixtrading.org/standards/sbe-online): заголовок messageHeader (blockLength, templateId, schemaId,
//! version — uint16), корневой блок длиной blockLength из заголовка (поля новых версий в конце блока пропускаются —
//! «schema extension mechanism», faqs/sbe_faq.md «Regarding Legacy support»), группы groupSize16Encoding (blockLength
//! uint16, numInGroup uint16), затем varString8 symbol (длина uint8 + UTF-8).
//! Официальный генератор (faqs/sbe_faq.md «Generate SBE decoders» — real-logic simple-binary-encoding, та же версия
//! 1.35.6, что у официального binance-sbe-rust-sample-app) на stream_1_0.xml даёт для DepthDiffStreamEvent:
//! SBE_TEMPLATE_ID 10003, SBE_BLOCK_LENGTH 26, SBE_SCHEMA_ID 1, смещения eventTime 0, firstBookUpdateId 8,
//! lastBookUpdateId 16, priceExponent 24, qtyExponent 25, в группе price 0, qty 8 (длина элемента 16). Сгенерированный
//! код не вставлен в проект: в нём нет проверок длины (короткий кадр — паника задачи), здесь разбор с проверками.
//! Число — mantissa64 × 10^exponent8 («64-bit mantissa for a decimal floating point number», «Exponent for a decimal
//! floating point number»; faqs/sbe_faq.md «Decimal field encoding»: мантисса и порядок — отдельные поля).

use rust_decimal::Decimal;

use crate::core::types::PriceLevel;

/// schemaId схемы stream_1_0.xml (атрибут id="1"). Другой schemaId — несовместимая схема (faqs/sbe_faq.md: «id is
/// incremented when a breaking change is introduced»).
pub const SCHEMA_ID: u16 = 1;
/// templateId сообщения DepthDiffStreamEvent (id="10003" в stream_1_0.xml).
pub const DEPTH_DIFF: u16 = 10003;
/// Длина messageHeader: 4 поля uint16 (stream_1_0.xml, composite messageHeader).
const HEADER_LEN: usize = 8;
/// Длина корневого блока DepthDiffStreamEvent версии 0: 3 × int64 + 2 × int8 (генератор: SBE_BLOCK_LENGTH 26).
const DIFF_BLOCK: usize = 26;
/// Длина заголовка группы groupSize16Encoding: uint16 + uint16.
const GROUP_HEADER: usize = 4;
/// Длина элемента группы bids / asks версии 0: price int64 + qty int64.
const LEVEL_LEN: usize = 16;

/// Изменения стакана одного кадра DepthDiffStreamEvent (уровни — как пришли, количество 0 — убрать уровень).
#[derive(Debug, Clone, PartialEq)]
pub struct Diff {
    /// eventTime, микросекунды UTC.
    pub event_us: i64,
    /// firstBookUpdateId — U «How to manage a local order book correctly».
    pub first: i64,
    /// lastBookUpdateId — u.
    pub last: i64,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    /// symbol как прислала биржа.
    pub symbol: String,
}

/// Что в кадре.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Изменения стакана.
    Diff(Diff),
    /// Другое сообщение схемы — его templateId (не разбирается).
    Other(u16),
}

/// Чтение кадра с проверкой длины.
struct Rd<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Rd<'a> {
    /// Следующие n байт. Ошибка — кадр короче.
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.b.len());
        let Some(end) = end else {
            return Err(format!(
                "кадр SBE короче ожидаемого: нужно {n} байт с {}, всего {}",
                self.pos,
                self.b.len()
            ));
        };
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// uint16 little-endian. Ошибка — кадр короче.
    fn u16(&mut self) -> Result<u16, String> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
}

/// int64 little-endian по смещению среза (длина среза проверена вызывающим). Ошибок нет.
fn i64_at(s: &[u8], at: usize) -> i64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&s[at..at + 8]);
    i64::from_le_bytes(a)
}

/// Что делает: разбирает двоичный кадр. Принимает: байты кадра WebSocket. Возвращает: Frame.
/// Ошибки: текст — кадр короче полей схемы, schemaId не 1, длина блока меньше версии 0, число вне Decimal, symbol не
/// UTF-8, лишние байты в конце.
pub fn decode(buf: &[u8]) -> Result<Frame, String> {
    let mut r = Rd { b: buf, pos: 0 };
    let block = r.u16()? as usize;
    let template = r.u16()?;
    let schema = r.u16()?;
    let _version = r.u16()?;
    debug_assert_eq!(r.pos, HEADER_LEN);
    if schema != SCHEMA_ID {
        return Err(format!(
            "schemaId {schema}: декодер написан по stream_1_0.xml (schemaId {SCHEMA_ID}) — нужна новая схема"
        ));
    }
    if template != DEPTH_DIFF {
        return Ok(Frame::Other(template));
    }
    if block < DIFF_BLOCK {
        return Err(format!(
            "blockLength {block} меньше {DIFF_BLOCK} (DepthDiffStreamEvent версии 0)"
        ));
    }
    let root = r.take(block)?;
    let price_exp = root[24] as i8;
    let qty_exp = root[25] as i8;
    let bids = group(&mut r, price_exp, qty_exp, "bids")?;
    let asks = group(&mut r, price_exp, qty_exp, "asks")?;
    let len = r.take(1)?[0] as usize;
    let symbol = std::str::from_utf8(r.take(len)?)
        .map_err(|_| "symbol не UTF-8".to_string())?
        .to_string();
    if r.pos != buf.len() {
        return Err(format!(
            "после symbol ещё {} байт — в схеме 1:0 их нет",
            buf.len() - r.pos
        ));
    }
    Ok(Frame::Diff(Diff {
        event_us: i64_at(root, 0),
        first: i64_at(root, 8),
        last: i64_at(root, 16),
        bids,
        asks,
        symbol,
    }))
}

/// Группа уровней groupSize16Encoding. Принимает: чтение, порядки цены и количества, имя стороны.
/// Возвращает: уровни (количество 0 — оставлено, это «убрать уровень»). Ошибки: как decode.
fn group(r: &mut Rd<'_>, pe: i8, qe: i8, side: &str) -> Result<Vec<PriceLevel>, String> {
    let h = r.take(GROUP_HEADER)?;
    let block = u16::from_le_bytes([h[0], h[1]]) as usize;
    let n = u16::from_le_bytes([h[2], h[3]]) as usize;
    if block < LEVEL_LEN {
        return Err(format!(
            "{side}: blockLength группы {block} меньше {LEVEL_LEN}"
        ));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let e = r.take(block)?;
        out.push(PriceLevel {
            price: dec(i64_at(e, 0), pe).map_err(|m| format!("{side}: цена {m}"))?,
            qty: dec(i64_at(e, 8), qe).map_err(|m| format!("{side}: количество {m}"))?,
        });
    }
    Ok(out)
}

/// Что делает: mantissa × 10^exponent без потери точности. Принимает: мантиссу, порядок.
/// Возвращает: Decimal. Ошибки: текст — не помещается в Decimal (порядок меньше −28 или переполнение).
pub fn dec(mantissa: i64, exponent: i8) -> Result<Decimal, String> {
    let bad = || format!("{mantissa}e{exponent} не помещается в Decimal");
    if exponent <= 0 {
        return Decimal::try_new(mantissa, (-(exponent as i32)) as u32).map_err(|_| bad());
    }
    let pow = 10i64.checked_pow(exponent as u32).ok_or_else(bad)?;
    Decimal::from(mantissa)
        .checked_mul(Decimal::from(pow))
        .ok_or_else(bad)
}
