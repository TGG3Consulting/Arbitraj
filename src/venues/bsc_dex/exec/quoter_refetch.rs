//! Файл: venues/bsc_dex/exec/quoter_refetch.rs
//! Что делает: решение 382 (требование Тиграна 17.11: «сделка не должна срываться из-за того, что НАША память пула
//! неверна») — сверка с квотером перед сделкой (решение 261, exec/precheck.rs) сравнивает расчёт по ПАМЯТИ бота с
//! квотером на «latest» (quote/quoter.rs out): это разные блоки, и неверная память (событие ещё не применено или
//! пропущено, карта тиков разошлась) выглядит как «квотер меньше расчёта» — сделка срывалась (живой бот 17.11: ESIM
//! 7,2 %, BANANAS31 ×8, VELO ×7). Теперь при таком расхождении (только Judged::Diff против нас — is_quoter_diff):
//! 1. голова H — свежий номер блока из опроса (node/heads.rs fresh, без запроса к узлу);
//! 2. ВСЕ пулы доли перечитываются на H тем же загрузчиком, что у периодической сверки (pools/load.rs refresh; в реестр
//!    НЕ пишется — запись реестра ведёт только сессия пачки);
//! 3. чем память отличалась от узла — строка в журнал dex.memory_refetch (Warn; exec/quoter_refetch_note.rs); память не
//!    равна узлу — пул ставится на ПОЛНОЕ чтение своей сессией (pools/settle_reread.rs: до него недостоверен);
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
//! exec/quoter_refetch_note.rs, pools/settle_reread.rs, quote/mod.rs (Quoting).
//! Настройки: через вызываемые — feed.stale_after_ms, настройки чтения пула (dex.tick_words, dex.tick_lens и др.).
//! Журнал: dex.memory_refetch (Warn) — расчёт по памяти разошёлся с квотером, пулы перечитаны на H, чем память отличалась.

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::{DexRoute, PoolRef};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::super::check::hook_batch::RouteFresh;
use super::super::pools::load;
use super::super::pools::registry::Entry;
use super::super::quote::Quoting;
use super::super::quote::via::Via;
use super::quoter_refetch_note;

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

/// Пулы доли на голове H (см. шапку, п. 1–3). Принимает: окружение котировки, пулы доли, исходный отказ сверки.
/// Возвращает: окружение с перечитанными пулами (Quoting.pinned). Ошибки: исходный отказ с припиской причины — нет
/// свежей головы, пула нет в реестре, чтение пула на H не удалось (узел, настройка).
pub async fn pinned(q: &Quoting, pools: &[PoolRef], first: GatewayError) -> GatewayResult<Quoting> {
    let ctx = &q.ctx;
    let fail = |why: String| GatewayError {
        message: format!(
            "{}; перечитать пулы с узла не удалось (решение 382): {why}",
            first.message
        ),
        ..first.clone()
    };
    let block = q.heads.fresh(ctx).map_err(|e| fail(e.message))?.number;
    let mut fresh = HashMap::with_capacity(pools.len());
    let mut notes = Vec::with_capacity(pools.len());
    for r in pools {
        if fresh.contains_key(r) {
            continue;
        }
        let mem = q
            .reg
            .get(r)
            .ok_or_else(|| fail("пул маршрута больше не в реестре".into()))?;
        let got = load::refresh(ctx, &q.tokens, &q.reg, &mem, block)
            .await
            .map_err(|e| {
                fail(format!(
                    "{} {}: {}",
                    mem.family.title, mem.ticker, e.message
                ))
            })?;
        let mut line = quoter_refetch_note::note(&mem, &got, block);
        // Память доказанно не равна узлу — пул на полное чтение своей сессией (pools/settle_reread.rs): до него не
        // котируется, следующая попытка и другие монеты через этот пул пойдут по верной памяти.
        if quoter_refetch_note::differs(&mem, &got) && q.reg.holds().reread(&q.reg, r, block) {
            line.push_str(" — пул поставлен на полное перечитывание сессией пачки");
        }
        notes.push(line);
        fresh.insert(r.clone(), got);
    }
    let msg = format!(
        "{}; пулы доли перечитаны на блоке {block}, пересчёт по ним и сверка с квотером на том же блоке \
         (решение 382): {}",
        first.message,
        notes.join(" | ")
    );
    ctx.log(Level::Warn, KIND, msg, None).await;
    Ok(Quoting {
        pinned: Some(Arc::new(RouteFresh {
            block,
            pools: Arc::new(fresh),
        })),
        ..q.clone()
    })
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
