//! Файл: venues/bsc_dex/pools/depth_key.rs
//! Что делает: ключ кэша глубины пула (решение 73, pools/depth_book.rs): глубина пересчитывается, если сменилось
//! состояние пула (блок, logIndex последнего учтённого события), настройки глубины (полоса, порог, гистерезис) или
//! курс встречного токена к USDT — состояние пулов «промежуточная — USDT» (пул с BNB, решение 52).
//! Кто вызывает: pools/depth_book.rs.
//! От чего зависит: registry.rs, tokens.rs, ctx.rs.
//! Настройки: нет (значения даёт вызывающий).

use rust_decimal::Decimal;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::registry::{Entry, Registry};

/// Что посчитано и на чём: состояние пула (блок, logIndex), настройки (полоса, порог, гистерезис) и состояние пулов
/// «промежуточная — USDT», по которым глубина переводится в USDT (пусто — встречный токен USDT).
#[derive(Debug, Clone, PartialEq)]
pub struct Key {
    at: (u64, Option<u64>),
    band: Decimal,
    min: Decimal,
    hyst: Decimal,
    rate: Vec<(u64, Option<u64>, bool)>,
}

impl Key {
    /// Ключ пула `e` при настройках полосы, порога и гистерезиса. Ошибок нет.
    pub fn new(
        ctx: &Ctx,
        reg: &Registry,
        tokens: &Tokens,
        e: &Entry,
        band: Decimal,
        min: Decimal,
        hyst: Decimal,
    ) -> Key {
        Key {
            at: (e.synced_block, e.synced_log),
            band,
            min,
            hyst,
            rate: rate_key(ctx, reg, tokens, e),
        }
    }
}

/// Состояние пулов «промежуточная — USDT» (решение 52), по которым глубина переводится в USDT: (блок, logIndex,
/// достоверен). Встречный токен — USDT или USDT не известен — пусто. Ошибок нет.
fn rate_key(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    e: &Entry,
) -> Vec<(u64, Option<u64>, bool)> {
    let Ok(usdt) = tokens.usdt(ctx) else {
        return Vec::new();
    };
    if e.usdt == usdt {
        return Vec::new();
    }
    let mut v: Vec<_> = reg
        .by_pair(e.usdt, usdt)
        .iter()
        .map(|x| (x.synced_block, x.synced_log, x.valid))
        .collect();
    v.sort_unstable();
    v
}
