//! Файл: venues/bitget/book_decided.rs
//! Что делает: решение 217 (исправление по проверке) — снимок стакана Bitget для quote_snapshot: тот же стакан, по
//! которому котировали. Стакан потока (book.rs freeze; после котировки исполнения по REST, решение 201, в нём уже
//! снимок REST — put_rest); у пары без стакана потока — снимок REST последней котировки исполнения (book.rs put_rest
//! запоминает его отдельно), если он не старше feed.stale_after_ms. Без запросов в пути сделки: нет ни того, ни
//! другого — «стакан устарел» (расчёт «без потолка» — «не посчитано»).
//! Кто вызывает: mod.rs (quote_snapshot).
//! От чего зависит: book.rs (Books, freeze, frozen), fees.rs, ctx.rs, gateway/frozen.rs.
//! Настройки: feed.stale_after_ms [bitget]; через book.rs freeze — venue.book_stale_ms.

use crate::core::types::{Asset, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::frozen::FrozenBook;

use super::book::{Books, frozen, now_ms};
use super::ctx::Ctx;
use super::fees::Fees;

impl Books {
    /// Решение 217 (исправление по проверке): снимок для quote_snapshot — тот же стакан, по которому котировали: стакан
    /// потока (freeze; после котировки исполнения по REST, решение 201, в нём уже снимок REST — put_rest); стакана
    /// потока у пары нет или он устарел — снимок REST последней котировки исполнения, если он не старше
    /// feed.stale_after_ms. Без запросов. Принимает и ошибки: как freeze (ошибка устаревания — от стакана потока).
    pub fn freeze_decided(
        &self,
        ctx: &Ctx,
        fees: &Fees,
        symbol: &str,
        asset: &Asset,
        side: Side,
    ) -> GatewayResult<FrozenBook> {
        let e = match self.freeze(ctx, fees, symbol, asset, side) {
            Err(e) if e.code == ErrorCode::StaleData => e,
            r => return r,
        };
        let stale_ms = ctx.int("feed.stale_after_ms")? as i64;
        let rest = self
            .rest
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&asset.ticker)
            .filter(|(_, at)| now_ms() - at <= stale_ms)
            .map(|(b, _)| b.clone());
        match rest {
            Some(book) => frozen(ctx, fees, symbol, asset, side, book),
            None => Err(e),
        }
    }
}
