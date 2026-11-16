//! Файл: venues/bsc_dex/pools/prefetch_read.rs
//! Что делает: решение 325 — перечитывание пула ПО СОБЫТИЮ (off_map, Refresh — refresh_event.rs) для ЛЮБОГО вида пула
//! с ответами, прочитанными заранее: 1) одним обращением к узлу (eth_call к Multicall3.aggregate3; больше
//! dex.load_batch вызовов — частями по нему) на блоке чтения читаются вызовы прошлого чтения этого пула
//! (prefetch_memo.rs) и вызовы, которые вид пула знает заранее (prefetch_plan.rs first: голова, слова карты вокруг
//! тика записи); 2) по уже прочитанным ответам вид добирает то, что от них зависит (prefetch_plan.rs more: слова
//! настоящего диапазона, тики по битам слов, цепочка списка тиков Integral) — следующим обращением, пока добирать есть
//! что (каждый вызов спрашивается заранее один раз); 3) прежний код чтения (load::refresh) идёт с книгой ответов на
//! задаче (node/prefetch.rs): что есть в книге — без узла, чего нет — к узлу как раньше. Итог — ровно итог
//! последовательного чтения на этом блоке: тот же код, тот же разбор, те же ошибки (ошибка заранее прочитанного
//! вызова в книгу не попадает — код чтения спросит узел сам и получит ту же ошибку). Вызовы кода чтения
//! запоминаются для следующего раза (prefetch_memo.rs).
//! Включаемый: dex.event_refresh_batch = да для DEX пула (область «DEX», id DEX) И заданы dex.multicall3, dex.load_batch.
//! Нет / не задана — путь не применяется (None), чтение прежнее.
//! Кто вызывает: pools/refresh_event.rs (read).
//! От чего зависит: node/prefetch.rs (Book, scope), node/call.rs (try_chunks, block_tag), node/multicall.rs (address),
//! load.rs (refresh), load_batch.rs (batch), prefetch_memo.rs, prefetch_plan.rs, registry.rs (Entry), tokens.rs.
//! Настройки: dex.event_refresh_batch [DEX], dex.multicall3, dex.load_batch [bsc_dex].
//!
//! Multicall3.aggregate3 — https://github.com/mds1/multicall3/blob/main/src/Multicall3.sol#L98 (node/multicall.rs).

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call::{self, block_tag};
use super::super::node::multicall;
use super::super::node::prefetch::{self, Book, Call};
use super::super::tokens::Tokens;
use super::registry::{Entry, Registry};
use super::{load, load_batch, prefetch_memo, prefetch_plan};

/// Включено ли для DEX пула (и есть ли Multicall3 и размер части). Возвращает: размер части пачки или None.
/// Ошибок нет: не задано — None.
pub fn size(ctx: &Ctx, e: &Entry) -> Option<usize> {
    if !ctx
        .dex_flag("dex.event_refresh_batch", e.family.id)
        .unwrap_or(false)
    {
        return None;
    }
    multicall::address(ctx)?;
    load_batch::batch(ctx).ok()
}

/// Перечитывает пул на блоке `at` с ответами, прочитанными заранее (см. шапку). Принимает: окружение, токены,
/// реестр, запись, блок. Возвращает: Some — перечитанная запись (как load::refresh); None — путь не применяется.
/// Ошибки: как у load::refresh (заранее прочитанное ошибок не даёт — только меньше обращений).
pub async fn refresh(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    e: &Entry,
    at: u64,
) -> GatewayResult<Option<Entry>> {
    let Some(part) = size(ctx, e) else {
        return Ok(None);
    };
    let tag = block_tag(at);
    let book = Book::new(&tag);
    let venue = ctx.venue();
    let mut ask = prefetch_memo::get(&venue, &e.pool.reference);
    ask.extend(prefetch_plan::first(ctx, e));
    loop {
        let calls = book.fresh(ask);
        if calls.is_empty() {
            break;
        }
        fetch(ctx, &book, &calls, &tag, part).await;
        ask = prefetch_plan::more(ctx, e, &book);
    }
    let res = prefetch::scope(book.clone(), load::refresh(ctx, tokens, reg, e, at)).await;
    if res.is_ok() {
        prefetch_memo::put(&venue, &e.pool.reference, book.used());
    }
    res.map(Some)
}

/// Одно обращение заранее (частями по `part` вызовов): ответы — в книгу. Сбой части — её вызовы не в книге (код
/// чтения спросит их у узла сам). Ошибок нет.
async fn fetch(ctx: &Ctx, book: &Book, calls: &[Call], tag: &str, part: usize) {
    let pieces = call::try_chunks(ctx, calls, tag, part).await;
    let answers: Vec<Result<alloy_primitives::Bytes, String>> =
        pieces.into_iter().map(|p| p.and_then(|r| r)).collect();
    book.fill(calls, &answers);
}
