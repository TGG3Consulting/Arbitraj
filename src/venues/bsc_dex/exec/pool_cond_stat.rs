//! Файл: venues/bsc_dex/exec/pool_cond_stat.rs
//! Что делает: решение 439 — статистика сделок цикла DEX ↔ DEX через исполнитель площадки DEX сети eip1559_l1
//! (Arbitrum) по режиму условия пулов на момент отправки, чтобы Тигран по ней решил, включать ли send.pool_cond_mode =
//! pools (решение Тиграна 20.11.2026: старт с off). На каждый итог отправки: число отправок; победа (в блоке без
//! отката); откат — число и газ в нативной монете (effectiveGasPrice × gasUsed квитанции, send/receipt.rs gas_bnb);
//! бесплатный отказ −32003 по условию пулов (send/sequencer_cond.rs REJECTED_POOLS); отказ −32003 по окну времени
//! (REJECTED без пулов); прочие итоги (сбой отправки, «не включена», «исход неизвестен») — только в отправках.
//!
//! Режим на момент отправки — режим в действии: pools — условие пулов прочитано для этой сделки (send/pool_cond_mode.rs
//! wanted: send.pool_cond_mode = pools и включённый канал sequencer с окном) и непусто (ревью 439 (5): ни один пул не
//! попал в условие — knownAccounts не ставится, отправка как off); иначе off (в том числе настройка не задана или окна
//! нет). Подсчёт и запись — в фоновой задаче после итога (сделка не ждёт); журнал send.pool_cond_stat на
//! каждый итог (без Telegram). Только у dex.gas_mode = eip1559_l1: у BSC статистики нет, поведение не меняется
//! (applies читается ДО отправки — решение 398, begin).
//! Кто вызывает: exec/cycle_exec.rs (begin, after), venue.rs (view, reset — экран «Каналы отправки»).
//! От чего зависит: exec/pool_cond_stat_store.rs, exec/txs.rs (Done), send/{gas_mode, pool_cond_mode, sequencer_cond,
//! receipt}.rs, ctx.rs, core/types (PoolCondStat).
//! Настройки: dex.gas_mode; view — send.pool_cond_mode для показа режима.

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{PoolCondCounts, PoolCondStat, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::send::gas_mode::{self, GasMode};
use super::super::send::pool_cond_mode::{self, PoolCondMode};
use super::super::send::sequencer_cond;
use super::pool_cond_stat_store::{change, get};
use super::txs::Done;

/// Итог отправки для статистики.
#[derive(Debug, Clone, Copy)]
enum Outcome {
    Win,
    /// Газ отката в нативной монете (None — квитанции нет или число не помещается).
    Revert(Option<Decimal>),
    RejectedPools,
    RejectedTime,
    Other,
}

/// Статистика — только у площадки dex.gas_mode = eip1559_l1. Ошибки: NotApplicable (400 в API) с причиной.
pub fn applies(ctx: &Ctx) -> GatewayResult<()> {
    let why = match gas_mode::read(ctx) {
        Ok(GasMode::Eip1559L1) => return Ok(()),
        Ok(_) => "статистика условия пулов — только при dex.gas_mode = eip1559_l1".to_string(),
        Err(e) => format!("статистика условия пулов: модель газа не прочитана — {}", e.message),
    };
    Err(ctx.err(ErrorCode::NotApplicable, why))
}

/// До отправки: режим для статистики. Принимает: окружение, есть ли у отправки непустое условие пулов (режим pools в
/// действии; ревью 439 (5)). Возвращает: режим; None — площадка не eip1559_l1 (статистики нет). Ошибок нет.
pub fn begin(ctx: &Ctx, pools: bool) -> Option<PoolCondMode> {
    applies(ctx).ok()?;
    Some(if pools {
        PoolCondMode::Pools
    } else {
        PoolCondMode::Off
    })
}

/// Итог отправки — в статистику и журнал send.pool_cond_stat, в фоновой задаче (сделка не ждёт). Принимает:
/// окружение, режим (begin; None — ничего не делается), id сделки, итог отправки. Ошибок нет.
pub fn after(ctx: &Ctx, mode: Option<PoolCondMode>, deal: &str, res: &GatewayResult<Done>) {
    let Some(mode) = mode else { return };
    let out = classify(res);
    let (ctx, deal) = (ctx.clone(), deal.to_string());
    tokio::spawn(async move {
        // Ревью 439 (6): статистика не загружена из базы — итог не учтён (причина — в журнале change).
        let Ok(now) = change(&ctx, |st| {
            let c = counts(st, mode);
            add(c, out);
            c.clone()
        })
        .await
        else {
            return;
        };
        let msg = format!(
            "Сделка {deal}: условие пулов {} — {}; всего в режиме: {}",
            mode.as_str(),
            outcome_text(out),
            counts_text(&now)
        );
        ctx.log(Level::Info, "send.pool_cond_stat", msg, None).await;
    });
}

/// Итог отправки → вид. Ошибок нет.
fn classify(res: &GatewayResult<Done>) -> Outcome {
    match res {
        Ok(d) if d.chain.outcome == TxOutcome::Included => Outcome::Win,
        Ok(d) if d.chain.outcome == TxOutcome::Reverted => {
            Outcome::Revert(d.receipt.as_ref().and_then(|r| r.gas_bnb()))
        }
        Err(e) if e.message.contains(sequencer_cond::REJECTED_POOLS) => Outcome::RejectedPools,
        Err(e) if e.message.contains(sequencer_cond::REJECTED) => Outcome::RejectedTime,
        _ => Outcome::Other,
    }
}

/// Счётчики режима. Ошибок нет.
fn counts(st: &mut PoolCondStat, mode: PoolCondMode) -> &mut PoolCondCounts {
    match mode {
        PoolCondMode::Off => &mut st.off,
        PoolCondMode::Pools => &mut st.pools,
    }
}

/// Прибавляет итог к счётчикам. Ошибок нет.
fn add(c: &mut PoolCondCounts, out: Outcome) {
    c.sends += 1;
    match out {
        Outcome::Win => c.wins += 1,
        Outcome::Revert(gas) => {
            c.reverts += 1;
            match gas {
                Some(g) => c.revert_gas += g,
                None => c.revert_gas_unknown += 1,
            }
        }
        Outcome::RejectedPools => c.rejected_pools += 1,
        Outcome::RejectedTime => c.rejected_time += 1,
        Outcome::Other => {}
    }
}

/// Текст итога для журнала. Ошибок нет.
fn outcome_text(out: Outcome) -> String {
    match out {
        Outcome::Win => "победа (в блоке без отката)".into(),
        Outcome::Revert(Some(g)) => format!("откат, газ {g} нативной монеты"),
        Outcome::Revert(None) => "откат, газ не учтён (нет квитанции)".into(),
        Outcome::RejectedPools => "бесплатный отказ по условию пулов (−32003)".into(),
        Outcome::RejectedTime => "отказ по окну времени (−32003)".into(),
        Outcome::Other => "прочий итог (сбой отправки, не включена, исход неизвестен)".into(),
    }
}

/// Счётчики одной строкой. Ошибок нет.
fn counts_text(c: &PoolCondCounts) -> String {
    format!(
        "отправок {}, побед {}, откатов {} (газ {}, не учтён у {}), отказов по условию \
         пулов {}, по окну времени {}",
        c.sends,
        c.wins,
        c.reverts,
        c.revert_gas,
        c.revert_gas_unknown,
        c.rejected_pools,
        c.rejected_time
    )
}

/// Статистика для экрана: сохранённая + режим send.pool_cond_mode сейчас (None — не задана, как off) и причина,
/// если значение не разобрано. Ошибки: NotApplicable — площадка не eip1559_l1.
pub async fn view(ctx: &Ctx) -> GatewayResult<PoolCondStat> {
    applies(ctx)?;
    let mut st = get(ctx).await;
    st.venue = ctx.place.id.to_string();
    st.mode = ctx.text("send.pool_cond_mode").ok();
    st.note = pool_cond_mode::read(ctx).err().map(|e| e.message);
    st.channels_note = pool_cond_mode::multi_note(ctx);
    Ok(st)
}

/// «Сбросить статистику»: счётчики обоих режимов — 0, отсчёт — с этого момента; прежние значения — в журнал.
/// Принимает: окружение, кто сбрасывает. Возвращает: статистику. Ошибки: NotApplicable — площадка не eip1559_l1;
/// Internal — статистика не загружена из SQLite (ревью 439 (6): сброс не выполнен, сохранённая не затёрта).
pub async fn reset(ctx: &Ctx, by: &str) -> GatewayResult<PoolCondStat> {
    applies(ctx)?;
    let was = change(ctx, |st| {
        let was = format!(
            "off: {}; pools: {}",
            counts_text(&st.off),
            counts_text(&st.pools)
        );
        st.off = PoolCondCounts::default();
        st.pools = PoolCondCounts::default();
        st.since = Some(Utc::now());
        was
    })
    .await
    .map_err(|why| ctx.err(ErrorCode::Internal, format!("сброс не выполнен: {why}")))?;
    let msg = format!("Статистика условия пулов сброшена ({by}); была — {was}");
    ctx.log(Level::Info, "send.pool_cond_stat", msg, None).await;
    view(ctx).await
}
