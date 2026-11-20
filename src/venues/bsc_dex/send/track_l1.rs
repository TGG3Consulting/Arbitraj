//! Файл: venues/bsc_dex/send/track_l1.rs
//! Что делает: решение 437 (1) — итог транзакции в сети с dex.gas_mode = eip1559_l1 (Arbitrum One) БЕЗ отмен.
//! Пула транзакций у сети нет, невключённую транзакцию секвенсор выбрасывает сам, а «пустая транзакция себе с тем же
//! nonce» либо сожжёт газ (исходная уже выброшена), либо получит «nonce too low» (исходная в блоке) — вытеснять нечего
//! (/home/claude/src-ext/reports/arb-cancel.md, раздел «Как правильно сделать в боте», п. 1). Поэтому:
//! - квитанция своим узлом на каждый новый блок (как send/track.rs) — «в блоке» (status 0x1) или «откат» (0x0);
//! - квитанции нет дольше send.drop_wait_ms ПО ВРЕМЕНИ от начала слежения (блоки Arbitrum идут только при
//!   транзакциях — arbitrum-docs how-arbitrum-works/deep-dives/sequencer-transaction-flow.mdx:95, поэтому не по
//!   блокам): eth_getTransactionCount на свежей голове H — nonce не занят → «выброшена» (NotIncluded без отмены: nonce
//!   свободен, сделка закрывается без повтора — правило «не исполнился — отменяем, больше не пробуем»; очередь
//!   кошелька свободна сразу, send/settle.rs); занят → ещё send.replaced_confirm_blocks блоков ищем свою квитанцию и
//!   eth_getTransactionByHash, потом «вытеснена» (как send/track.rs);
//!   Ревью 437, п. 1: «выброшена» — только по свежему своему узлу: голова получена не раньше feed.stale_after_ms
//!   назад (SendCfg.stale) и время её блока не раньше начала слежения + send.drop_wait_ms; иначе узел мог отстать от
//!   секвенсора (квитанции нет и nonce не вырос — у отставшего узла, а транзакция в блоке) — ждём дальше;
//!   ревью 438 (A): сначала caught_up и номер свежей головы H, затем квитанция и eth_getTransactionCount(from, H) — на
//!   той же голове (голова N+1, пришедшая после ответа о nonce на N, не делает включённую транзакцию «выброшенной»);
//!   ревью 438 (B): начало окна — время блока головы своего узла на момент отправки (не часы сервера: спешащие часы
//!   не дают caught_up никогда, отстающие — раньше срока); голов ещё не было — первая пришедшая;
//! - исход не установлен за send.queue_max_wait_s — «исход неизвестен» (очередь останавливается).
//!
//! Ответ отправки уже разобран до трекера (send/mod.rs classify): ошибка всех каналов и своего узла нет транзакции —
//! ChannelRejected (не исполнена, nonce свободен, решение 154); «nonce too low» — та же проверка своим узлом по хешу
//! (наша в блоке — к трекеру, иначе отказ); −32003 условной отправки — отказ (решение 436); таймаут/обрыв — «без
//! ясного ответа» → сюда, ждать по времени. «"pending" = "latest"» у Arbitrum — arbitrum-docs
//! arbitrum-essentials/arbitrum-vs-ethereum/nonce-management.mdx:64; пула нет — nitro-geth arbitrum/apibackend.go:729-745.
//! Решение 398: все настройки — в SendCfg до отправки (drop_wait, replaced_confirm, queue_max_wait, stale); здесь не
//! читаются (сторож tests/settings_before_send_guard.rs).
//! Кто вызывает: send/track.rs (resolve при SendCfg.drop_wait).
//! От чего зависит: node/{rpc, heads, head_age, call}.rs, send/receipt.rs, send/track_util.rs, ctx.rs.
//! Настройки: нет своих чтений (send.drop_wait_ms и прочие — через SendCfg).

use std::time::{Duration, Instant};

use alloy_primitives::U64;
use chrono::Utc;
use serde_json::json;

use crate::core::types::TxOutcome;
use crate::storage::journal::Level;

use super::super::node::call::block_tag;
use super::super::node::heads::Head;
use super::super::node::{head_age, rpc};
use super::receipt;
use super::track_util::{Quiet, Tracked, Watch, heads_rx, landed};

/// Ждёт итога без отмен (см. шапку). Принимает: что отслеживается, окно send.drop_wait_ms. Возвращает: итог.
/// Ошибок нет — сбои узла повторяются.
pub async fn resolve(w: Watch, window: Duration) -> Tracked {
    let ctx = &w.ctx;
    let started = Instant::now();
    // Ревью 438 (B): начало окна — время блока головы на момент отправки (читается при первой нужде).
    let mut start = Start {
        head: w.heads.latest().map(|h| h.number),
        secs: None,
    };
    let mut rx = heads_rx(&w.heads);
    let mut quiet = Quiet::new(ctx, w.hash);
    let ours = [w.hash];
    let mut taken_at: Option<u64> = None;
    loop {
        if started.elapsed() > w.cfg.queue_max_wait {
            let why = format!(
                "итог не установлен за send.queue_max_wait_s; последний сбой: {}",
                quiet.reason()
            );
            return Tracked::bare(TxOutcome::Pending, Some(why));
        }
        let head = step(&mut rx, &w, started, window).await;
        match receipt::find(ctx, &ours).await {
            Ok(Some((i, r))) => return landed(&w, &ours, i, r),
            Ok(None) => quiet.ok("receipt").await,
            Err(e) => {
                let why = format!("квитанция не прочитана: {}", e.message);
                quiet.warn("receipt", why).await;
                continue;
            }
        }
        if started.elapsed() < window {
            continue;
        }
        if let Some(at) = taken_at {
            // Nonce занят, своей квитанции нет: узел знает нашу транзакцию в блоке — ждём квитанцию дальше.
            match receipt::in_block(ctx, &ours).await {
                Ok(true) => continue,
                Ok(false) if head >= at + w.cfg.replaced_confirm => {
                    return Tracked::bare(TxOutcome::Replaced, None);
                }
                Ok(false) => continue,
                Err(e) => {
                    let why = format!("eth_getTransactionByHash: {}", e.message);
                    quiet.warn("by_hash", why).await;
                    continue;
                }
            }
        }
        // Ревью 438 (A): сначала свежая голова H, затем квитанция и nonce — на ней же.
        let at = match caught_up(&w, &mut start, window).await {
            Ok(Some(h)) => h,
            Ok(None) => continue,
            Err(why) => {
                quiet.warn("head_time", why).await;
                continue;
            }
        };
        match receipt::find(ctx, &ours).await {
            Ok(Some((i, r))) => return landed(&w, &ours, i, r),
            Ok(None) => {}
            Err(e) => {
                let why = format!("квитанция не прочитана: {}", e.message);
                quiet.warn("receipt", why).await;
                continue;
            }
        }
        let params = json!([w.from, block_tag(at)]);
        let count: U64 = match rpc::call(ctx, "eth_getTransactionCount", params).await {
            Ok(c) => c,
            Err(e) => {
                let why = format!("nonce кошелька на блоке {at} не прочитан: {}", e.message);
                quiet.warn("nonce", why).await;
                continue;
            }
        };
        if count.to::<u64>() > w.nonce {
            taken_at = Some(at);
            continue;
        }
        let msg = format!(
            "{} без квитанции за send.drop_wait_ms ({} мс), nonce {} не занят — выброшена секвенсором, \
             не исполнена (отмен у eip1559_l1 нет, повтора нет)",
            w.hash,
            window.as_millis(),
            w.nonce
        );
        ctx.log(Level::Warn, "send.tx_not_included", msg, None)
            .await;
        return Tracked::bare(TxOutcome::NotIncluded, None);
    }
}

/// Шаг ожидания: новый блок или конец окна send.drop_wait_ms — что раньше, но не дольше feed.stale_after_ms (SendCfg).
/// Поток блоков закрыт — сон того же шага (без пустого круга). Принимает: приёмник голов, слежение, начало, окно.
/// Возвращает: номер последней известной головы (0 — голов ещё не было). Ошибок нет.
async fn step(
    rx: &mut tokio::sync::watch::Receiver<Option<Head>>,
    w: &Watch,
    started: Instant,
    window: Duration,
) -> u64 {
    let left = window.saturating_sub(started.elapsed());
    let wait = if left.is_zero() {
        w.cfg.stale
    } else {
        left.min(w.cfg.stale)
    };
    if let Ok(Err(_)) = tokio::time::timeout(wait, rx.changed()).await {
        tokio::time::sleep(wait).await;
    }
    w.heads.latest().map_or(0, |h| h.number)
}

/// Ревью 438 (B): начало окна — голова своего узла на момент отправки и время её блока (секунды Unix).
struct Start {
    /// Номер головы при отправке; None — голов ещё не было (берётся первая пришедшая).
    head: Option<u64>,
    /// Время её блока; None — ещё не прочитано.
    secs: Option<u64>,
}

/// Время блока n: из памяти голов, иначе с узла (node/head_age.rs fetch). Ошибки: текст (узел).
async fn block_secs(w: &Watch, n: u64) -> Result<u64, String> {
    match w.heads.block_time().filter(|(b, _)| *b == n) {
        Some((_, t)) => Ok(t),
        None => head_age::fetch(&w.ctx, n, true)
            .await
            .map(|t| t.secs)
            .map_err(|e| format!("время блока {n}: {}", e.message)),
    }
}

/// Ревью 437, п. 1; ревью 438 (A, B): свой узел догнал секвенсор настолько, чтобы верить «квитанции нет, nonce не
/// вырос»: голова свежая (получена не раньше SendCfg.stale назад) и время её блока не раньше времени блока головы на
/// момент отправки + окно (оба — время сети, секунды с округлением вниз; часы сервера не участвуют). Принимает:
/// слежение, начало (меняется: номер и время читаются один раз), окно send.drop_wait_ms. Возвращает: Some(номер
/// головы H) — догнал, на H читать nonce; None — ещё нет. Ошибки: текст — время блока не прочитано (узел).
async fn caught_up(w: &Watch, start: &mut Start, window: Duration) -> Result<Option<u64>, String> {
    let Some(h) = w.heads.latest() else {
        return Ok(None);
    };
    let age = (Utc::now() - h.received_at).num_milliseconds();
    let stale = i64::try_from(w.cfg.stale.as_millis()).unwrap_or(i64::MAX);
    if !(0..=stale).contains(&age) {
        return Ok(None);
    }
    let begin = match start.secs {
        Some(s) => s,
        None => {
            let n = *start.head.get_or_insert(h.number);
            let s = block_secs(w, n).await?;
            start.secs = Some(s);
            s
        }
    };
    let now = block_secs(w, h.number).await?;
    let need = u128::from(begin) * 1000 + window.as_millis();
    Ok((u128::from(now) * 1000 >= need).then_some(h.number))
}
