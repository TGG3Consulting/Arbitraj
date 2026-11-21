//! Файл: venues/bsc_dex/exec/quoter_refetch.rs
//! Что делает: решение 382 (требование Тиграна 17.11: «сделка не должна срываться из-за того, что НАША память пула
//! неверна») — сверка с квотером перед сделкой (решение 261, exec/precheck.rs) сравнивает расчёт по ПАМЯТИ бота с
//! квотером на «latest» (quote/quoter.rs out): это разные блоки, и неверная память (событие ещё не применено или
//! пропущено, карта тиков разошлась) выглядит как «квотер меньше расчёта» — сделка срывалась (живой бот 17.11: ESIM
//! 7,2 %, BANANAS31 ×8, VELO ×7). Теперь при таком расхождении (только Judged::Diff против нас — is_quoter_diff):
//! 1. голова H — свежий номер блока из опроса (node/heads.rs fresh, без запроса к узлу);
//! 2. ВСЕ пулы доли перечитываются на H одновременно тем же загрузчиком, что у периодической сверки (pools/load.rs
//!    refresh; в реестр НЕ пишется — запись реестра ведёт только сессия пачки); графовый маршрут — пулы предторговой
//!    верификации на её блоке (Quoting.route_fresh), без нового чтения;
//! 3. чем память отличалась от узла на H — строка в журнал dex.memory_refetch (Warn; exec/quoter_refetch_note.rs). Это
//!    ещё не ошибка (память может отставать на блок, ревью Б1): фоном ошибка доказывается логами узла и сравнением на H
//!    (check/memory_proof.rs, exec/quoter_refetch_proof.rs), и только доказанная ошибка ставит пул на ПОЛНОЕ чтение
//!    сессией (pools/settle_reread.rs: до него недостоверен);
//! 4. вызывающий пересчитывает долю по этим записям (Quoting.pinned → exec/live.rs, exec/live_hops.rs) и сверяет с
//!    квотером на ТОМ ЖЕ блоке H (exec/quoter_refetch_check.rs) — дальше обычные проверки сделки (худшая цена ядра,
//!    минимумы, проба). Расходится и на одном блоке — это уже не память, а математика: отказ QuoteMismatch с
//!    пометкой блока (решение 261 в силе — односторонний стоп). Чтение не удалось — прежний отказ с причиной.
//!
//! Повторное перечитывание в той же попытке не делается (pinned уже задан — is_quoter_diff не срабатывает).
//! Все пути исполнения со сверкой квотером: прямой (exec/trade_direct_prep.rs), умный и графовый «один UR»
//! (exec/smart_legs.rs — MixedQuoter), исполнитель (exec/executor/prep_refetch.rs — хопы Maverick), сухая сборка и
//! симуляция продажи до покупки (exec/dry_build.rs). Бандл свопов и циклы DEX ↔ DEX квотером не сверяются (решение 261).
//! Кто вызывает: перечисленные выше; exec/live.rs, exec/live_hops.rs (entry).
//! От чего зависит: pools/load.rs (refresh), pools/registry.rs, node/heads.rs, check/hook_batch.rs (RouteFresh),
//! exec/quoter_refetch_note.rs, pools/settle_reread.rs, quote/mod.rs (Quoting), pools/fusion_book.rs (isolated —
//! решение 463: оракул THENA Fusion чтения не в книгу).
//! Настройки: через вызываемые — feed.stale_after_ms, настройки чтения пула (dex.tick_words, dex.tick_lens и др.).
//! Журнал: dex.memory_refetch (Warn) — расчёт по памяти разошёлся с квотером, пулы перечитаны на H, чем память отличалась.

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::{DexRoute, PoolRef};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::super::check::hook_batch::RouteFresh;
use super::super::pools::registry::Entry;
use super::super::pools::{fusion_book, load};
use super::super::quote::Quoting;
use super::super::quote::via::Via;
use super::{quoter_refetch_note, quoter_refetch_proof};

/// Вид записи журнала.
pub const KIND: &str = "dex.memory_refetch";

/// Признаки текста отказа сверки с квотером по цифрам: имя допуска и слово «расхождение» — их пишут все сверки перед
/// сделкой: квотер пула (quote/quoter.rs Judged::reason у Judged::Diff), get_dy крючка Infinity Stable
/// (quote/quoter_extra.rs — та же строка), MixedQuoter (quote/mixed_quoter.rs).
const DIFF_MARKS: [&str; 2] = ["(dex.quoter_max_diff_pct)", "расхождение "];
/// Признак «расхождение не считается» (MixedQuoter без цифр) — это не память, перечитывание не поможет.
const NO_NUMBERS: &str = "не считается";

/// Отказ — расхождение расчёта с квотером против нас по цифрам (квотер пула, get_dy, MixedQuoter), а не цена хуже
/// допустимой, «сравнить нельзя» или «не считается». Принимает: ошибку. Ошибок нет.
pub fn is_quoter_diff(e: &GatewayError) -> bool {
    e.code == ErrorCode::QuoteMismatch
        && DIFF_MARKS.iter().all(|m| e.message.contains(m))
        && !e.message.contains(NO_NUMBERS)
}

/// Перечитывать ли пулы после этого отказа: расхождение с квотером и пулы ещё не перечитывались в этой попытке.
/// Ошибок нет.
pub fn wants(q: &Quoting, e: &GatewayError) -> bool {
    q.pinned.is_none() && is_quoter_diff(e)
}

/// Запись пула для расчёта доли: перечитанная (Quoting.pinned), иначе из памяти. None — пула нет в реестре. Ошибок нет.
pub fn entry(q: &Quoting, r: &PoolRef) -> Option<Entry> {
    q.pinned
        .as_ref()
        .and_then(|f| f.pools.get(r).cloned())
        .or_else(|| q.reg.get(r))
}

/// Пулы всех долей маршрута (для pinned). Ошибок нет.
pub fn route_pools(route: &DexRoute) -> Vec<PoolRef> {
    route
        .legs
        .iter()
        .flat_map(|l| l.hops.iter().map(|h| h.pool.clone()))
        .collect()
}

/// Тот же расчёт путей (монета, сторона, налоги, decimals), но по окружению с перечитанными пулами. Ошибок нет.
pub fn via_on<'b>(v: &Via<'b>, q: &'b Quoting) -> Via<'b> {
    Via {
        q,
        coin: v.coin,
        usdt: v.usdt,
        side: v.side,
        taxes: v.taxes,
        coin_dec: v.coin_dec,
        usdt_dec: v.usdt_dec,
    }
}

/// Блок перечитанных пулов (None — расчёт по памяти, квотер на «latest»). Ошибок нет.
pub fn block(q: &Quoting) -> Option<u64> {
    q.pinned.as_ref().map(|f| f.block)
}

/// Пулы доли на голове H (см. шапку, п. 1–3). Ревью Н2: пулы читаются одновременно (один блок); у графового маршрута
/// пулы, уже перечитанные предторговой верификацией (Quoting.route_fresh), берутся как есть — квотер на её блоке.
/// Ревью Б1: расхождение памяти с узлом на H ошибку не доказывает (память может отставать на блок) — доказательство на
/// логам узла за [блок памяти, H] идёт фоном (exec/quoter_refetch_proof.rs, check/memory_proof.rs), отметку «нужно полное чтение» ставит только оно.
/// Принимает: окружение котировки, пулы доли, исходный отказ сверки. Возвращает: окружение с перечитанными пулами
/// (Quoting.pinned). Ошибки: исходный отказ с припиской причины — нет свежей головы, пула нет в реестре, чтение не
/// удалось (узел, настройка).
pub async fn pinned(q: &Quoting, pools: &[PoolRef], first: GatewayError) -> GatewayResult<Quoting> {
    let ctx = &q.ctx;
    let fail = |why: String| GatewayError {
        message: format!(
            "{}; перечитать пулы с узла не удалось (решение 382): {why}",
            first.message
        ),
        ..first.clone()
    };
    let mut uniq: Vec<PoolRef> = Vec::with_capacity(pools.len());
    for r in pools {
        if !uniq.contains(r) {
            uniq.push(r.clone());
        }
    }
    let mems = uniq
        .iter()
        .map(|r| {
            q.reg
                .get(r)
                .ok_or_else(|| fail("пул маршрута больше не в реестре".into()))
        })
        .collect::<GatewayResult<Vec<Entry>>>()?;
    let (block, got) = match verified(q, &uniq) {
        Some(v) => v,
        None => {
            let block = q.heads.fresh(ctx).map_err(|e| fail(e.message))?.number;
            // Ревью 2 (Н4): в причине — пул, который не прочитался.
            let reads = mems.iter().map(|m| async move {
                // Решение 463 (повторное ревью t236): оракул THENA Fusion этого чтения — не в книгу.
                fusion_book::isolated(load::refresh(ctx, &q.tokens, &q.reg, m, block))
                    .await
                    .map_err(|e| format!("{} {}: {}", m.family.title, m.ticker, e.message))
            });
            let got = futures_util::future::try_join_all(reads)
                .await
                .map_err(fail)?;
            (block, got)
        }
    };
    let notes: Vec<String> = mems
        .iter()
        .zip(&got)
        .map(|(m, g)| quoter_refetch_note::note(m, g, block))
        .collect();
    // Ревью Б1: ошибка памяти — только доказанная (логи узла, сравнение на H), фоном, не задерживая сделку.
    for (m, g) in mems.iter().zip(&got) {
        quoter_refetch_proof::spawn(ctx, (&q.tokens, &q.reg), m, (block, g), (KIND, true));
    }
    let msg = format!(
        "{}; пулы доли перечитаны на блоке {block}, пересчёт по ним и сверка с квотером на том же блоке \
         (решение 382); память против блока {block} (отставание ещё не ошибка — доказательство фоном): {}",
        first.message,
        notes.join(" | ")
    );
    ctx.log(Level::Warn, KIND, msg, None).await;
    Ok(Quoting {
        pinned: Some(Arc::new(RouteFresh {
            block,
            pools: Arc::new(uniq.into_iter().zip(got).collect::<HashMap<_, _>>()),
        })),
        ..q.clone()
    })
}

/// Ревью Н2: пулы доли, уже перечитанные предторговой верификацией графа (Quoting.route_fresh), — блок и записи; None
/// — верификации не было или в ней нет хоть одного пула. Ошибок нет.
fn verified(q: &Quoting, pools: &[PoolRef]) -> Option<(u64, Vec<Entry>)> {
    let f = q.route_fresh.as_ref()?;
    let got = pools
        .iter()
        .map(|r| f.pools.get(r).cloned())
        .collect::<Option<Vec<_>>>()?;
    Some((f.block, got))
}

/// Отказ сверки уже на перечитанных пулах — математика, не память: к тексту дописывается блок. Прочие ошибки — как
/// есть. Принимает: окружение (с pinned), ошибку. Ошибок нет (возвращает ошибку).
pub fn on_block(q: &Quoting, e: GatewayError) -> GatewayError {
    match block(q) {
        Some(b) if is_quoter_diff(&e) => GatewayError {
            message: format!(
                "{} — на одном блоке {b}: пулы перечитаны с узла, расчёт по ним и квотер расходятся (решение 382)",
                e.message
            ),
            ..e
        },
        _ => e,
    }
}
