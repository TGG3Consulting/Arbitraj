//! Файл: venues/bsc_dex/check/quoter_compare.rs
//! Что делает: сверка пула на одной сумме — локальная математика (за вычетом влияния крючка) против квотера своей DEX
//! НА ОДНОМ БЛОКЕ (решение 261, часть 2): блок, на котором состояние пула в памяти равно состоянию сети
//! (check/quoter_block.rs), и на нём же eth_call основного и дополнительного квотера (quote/quoter.rs out_at,
//! quote/quoter_extra.rs). Расхождение больше dex.quoter_max_diff_pct (по модулю — это проверка математики) — пауза
//! стороны (Cmp::Pause). Блок не определён или у узла нет состояния блока — «не сверено (блок)» (Cmp::NoBlock): не
//! ошибка математики, сторона не исключается. Пустой пул, неполный локальный расчёт, откат квотера «не хватает
//! ликвидности» (quote/emptiness.rs, quote/quoter_revert.rs) — честная причина (Cmp::Fail), не пауза.
//! Перенесено из quote/quoter.rs (compare) решением 261.
//! Кто вызывает: check/quoter_verify.rs.
//! От чего зависит: check/quoter_block.rs, quote/quoter.rs, quote/quoter_extra.rs, quote/pool_quote.rs,
//! quote/emptiness.rs, quote/quoter_revert.rs, node/call.rs (block_tag).
//! Настройки: dex.quoter_max_diff_pct [bsc_dex]; через вызываемые — dex.quoter / dex.router [вид пула].

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::node::call;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::super::quote::quoter::Judged;
use super::super::quote::{emptiness, pool_quote, quoter, quoter_extra, quoter_revert};
use super::quoter_block::{self, NOT_CHECKED};

/// Начало причины паузы стороны: расхождение с квотером на одном блоке (по ней — алерты, check/quoter_pause.rs).
pub const PAUSED: &str = "пауза: расчёт разошёлся с квотером на одном блоке";

/// Цифры расхождения на блоке: наш выход (за вычетом крючка) и выход квотера в минимальных единицах токена выхода,
/// расхождение % и допуск %.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nums {
    pub local: U256,
    pub remote: U256,
    pub pct: Decimal,
    pub limit: Decimal,
}

/// Итог сверки на одной сумме.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmp {
    /// В пределах допуска на блоке.
    Ok { block: u64 },
    /// Расхождение больше допуска на одном блоке: блок, текст причины, цифры (None — расхождение get_dy, цифры в тексте).
    Pause {
        block: u64,
        why: String,
        nums: Option<Nums>,
    },
    /// Иная причина (пустой пул, не хватает ликвидности, сравнить нельзя, отказ get_dy) — сторона исключена, без паузы.
    Fail(String),
    /// Блок не определён / у узла нет состояния блока — «не сверено (блок)», сторона не исключается.
    NoBlock(String),
}

/// Итог сверки по Judged на блоке (чистая, тестируется на цифрах живого случая). Ошибок нет.
pub fn judged(j: Judged, block: u64) -> Cmp {
    let why = j.reason().unwrap_or_default();
    match j {
        Judged::Within => Cmp::Ok { block },
        Judged::Diff {
            local,
            remote,
            pct,
            limit,
        } => Cmp::Pause {
            block,
            why: format!("{PAUSED} {block}: {why}"),
            nums: Some(Nums {
                local,
                remote,
                pct,
                limit,
            }),
        },
        Judged::Bad(why) => Cmp::Fail(why),
    }
}

/// «Не сверено (блок)» с причиной. Ошибок нет.
pub fn no_block(why: impl std::fmt::Display) -> Cmp {
    Cmp::NoBlock(format!("{NOT_CHECKED}: {why}"))
}

/// Сверка пула на одной сумме и одном блоке (см. шапку). Принимает: пулы, запись, токен входа, сумму, влияние крючка
/// на эту сторону, % (0 — крючка B/C нет). Возвращает: Cmp. Ошибки: настройки, узел (кроме «нет состояния блока»).
pub async fn compare(
    p: &Pools,
    e: &Entry,
    token_in: Address,
    amount: U256,
    hook_pct: Decimal,
) -> GatewayResult<Cmp> {
    let ctx = &p.ctx;
    if emptiness::empty(e) {
        return Ok(Cmp::Fail(emptiness::EMPTY.into()));
    }
    // Настройки — до обращения к узлу (ревью #65): без допуска вызов квотера бесполезен.
    ctx.dec("dex.quoter_max_diff_pct")?;
    let local = pool_quote::swap(ctx, e, token_in, true, amount)?;
    if !local.complete {
        return Ok(Cmp::Fail(emptiness::short(emptiness::SHORT_LOCAL)));
    }
    let block = match quoter_block::block(p, e).await {
        Ok(b) => b,
        Err(why) => return Ok(no_block(why)),
    };
    let tag = call::block_tag(block);
    // Решение «122 (Infinity Stable, ответы)»: у Infinity Stable ещё get_dy крючка — параллельно, на том же блоке.
    let (main, extra) = tokio::join!(
        quoter::out_at(ctx, e, token_in, amount, &tag),
        quoter_extra::check(ctx, e, token_in, amount, local.amount_out, (&tag, false))
    );
    match extra {
        Err(err) if err.code == ErrorCode::StaleData => return Ok(no_block(err.message)),
        Err(err) => return Err(err),
        // Расхождение get_dy больше допуска на том же блоке — тоже ошибка математики (пауза); отказ вызова — нет.
        Ok(Err(why)) if why.contains("(dex.quoter_max_diff_pct)") => {
            return Ok(Cmp::Pause {
                block,
                why: format!("{PAUSED} {block}: {why}"),
                nums: None,
            });
        }
        Ok(Err(why)) => return Ok(Cmp::Fail(why)),
        Ok(Ok(_)) => {}
    }
    let remote = match main {
        Ok(r) => r,
        Err(err) => {
            return match quoter_revert::liquidity_short(&err) {
                Some(why) => Ok(Cmp::Fail(emptiness::short(why))),
                None if err.code == ErrorCode::StaleData => Ok(no_block(err.message)),
                None => Err(err),
            };
        }
    };
    Ok(judged(
        quoter::judge(ctx, local.amount_out, remote, hook_pct)?,
        block,
    ))
}
