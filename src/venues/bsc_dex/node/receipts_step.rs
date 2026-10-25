//! Файл: venues/bsc_dex/node/receipts_step.rs
//! Что делает: один шаг ленты квитанций (решения 68, 69, 72): начало ленты, следующий блок из конвейера (строго по
//! порядку, проверка цепочки parentHash) или откат при реорге; отставание больше bsc.receipts_max_lag_blocks — отметка
//! «данные пулов отстают» (догон без сброса); блок не отдаёт ни один дошедший до него узел — невосполнимый пропуск.
//! Кто вызывает: node/receipts_task.rs.
//! От чего зависит: receipts.rs (Feed), receipts_chain.rs, receipts_fetch.rs, receipts_io.rs, receipts_lag.rs,
//! receipts_pipe.rs, ctx.rs.
//! Настройки: bsc.receipts_max_lag_blocks, bsc.receipts_reorg_depth, bsc.receipts_ahead_blocks.

use super::super::ctx::Ctx;
use super::receipts::Feed;
use super::receipts_chain::{Block, Chain, logs_of};
use super::receipts_fetch::fetch_block;
use super::receipts_io::{Fail, header, rewind};
use super::receipts_lag::Lag;
use super::receipts_pipe::Pipe;
use super::receipts_task::Got;

/// Один шаг: начало ленты, один следующий блок или откат одного отменённого. Следующие блоки до
/// bsc.receipts_ahead_blocks вперёд уже запрашиваются параллельно (конвейер, решение 69), забирается и раздаётся строго
/// следующий по номеру; не пришёл — он же снова, пришедшие после него ждут. Возвращает: true — дошли до `target`.
/// Ошибки: Fail.
pub(super) async fn step(
    feed: &Feed,
    ctx: &Ctx,
    chain: &mut Chain,
    pipe: &mut Pipe<Got>,
    lag: &mut Lag,
    target: u64,
) -> Result<bool, Fail> {
    let set = |name: &str| ctx.int(name).map_err(|e| Fail::Break(e.message));
    let (max_lag, depth) = (
        set("bsc.receipts_max_lag_blocks")?,
        set("bsc.receipts_reorg_depth")?,
    );
    let ahead = set("bsc.receipts_ahead_blocks")?;
    let depth = usize::try_from(depth).unwrap_or(usize::MAX);
    let Some(top) = chain.top().cloned() else {
        // Начало: подписки, сделанные после этого, получат всё, что позже `target`.
        pipe.clear();
        let h = header(ctx, target).await?;
        chain.push(
            Block {
                number: target,
                hash: h.hash,
                logs: Vec::new(),
            },
            depth,
        );
        feed.deliver(&[], target);
        return Ok(true);
    };
    if target <= top.number {
        return Ok(true);
    }
    // Отстали больше bsc.receipts_max_lag_blocks — не сброс, а догон (решение 72): состояние пулов остаётся,
    // котировка и сделка запрещены отметкой «данные пулов отстают на N блоков», пока лента не догнала.
    let behind = target - top.number;
    lag.mark(ctx, &feed.inner.lag, behind, max_lag).await;
    lag.report(ctx, behind, ahead).await;
    let n = top.number + 1;
    // Блок, на котором конвейер стоит, — гонкой на все узлы (решение 71).
    let hot = lag.hot();
    pipe.fill(n, target, ahead, |k| {
        fetch_block(ctx.clone(), feed.inner.nodes.clone(), k, hot == Some(k))
    });
    let got = pipe
        .take(n)
        .await
        .ok_or_else(|| Fail::Retry(format!("запрос блока {n} отменён")))?;
    let (h, r) = match got {
        Ok(x) => x,
        // Пропуск невосполним: гонка на все узлы, у каждого своя голова не ниже блока, а блок не отдаёт ни один
        // (receipts_fetch::nowhere) — полный сброс. Узел ещё не дошёл до блока — повтор (Retry, не Missing).
        Err(Fail::Missing(why)) => {
            return Err(Fail::Break(format!(
                "пропуск невосполним: блок {n} не отдаёт ни один узел, хотя их головы не ниже ({why}) — пулы недостоверны, лента заново{}",
                lag.stuck_text()
            )));
        }
        Err(e) => return Err(e),
    };
    if !chain.continues(h.parent_hash) {
        // Реорг: запрошенное вперёд могло быть от старой цепочки — всё заново после отката.
        pipe.clear();
        return rewind(feed, ctx, chain, n).await.map(|_| false);
    }
    lag.passed(n);
    let any = feed.deliver(&logs_of(&r), n);
    chain.push(
        Block {
            number: n,
            hash: h.hash,
            logs: any,
        },
        depth,
    );
    Ok(n == target)
}
