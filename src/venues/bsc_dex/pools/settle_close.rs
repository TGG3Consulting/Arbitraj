//! Файл: venues/bsc_dex/pools/settle_close.rs
//! Что делает: решение 357 — предел «в обновлении» (settle.rs Need::Active / Follow): недостающие логи блока пула так
//! и не пришли, а сеть ушла дальше — пул читается ЦЕЛИКОМ на голове (reload::load_one: состояние на конце блока головы,
//! valid = true; поздние логи блоков до головы отбрасываются по has_applied) и уходит ядру; не прочитался — недостоверен
//! и встаёт в очередь повтора пачки (stale_read::requeue → reload::retry на следующих головах, как решение 298).
//! Когда закрывать: новая голова `at` потока голов (node/heads.rs, сессия пачки — session.rs) ВЫШЕ блока отметки, и
//! канал событий пачки пуст — недостающим логам этого блока негде ждать: WebSocket присылает логи блока подряд, по
//! одному уведомлению на лог, без метки «блок кончился» (bnb-chain/bsc eth/filters/api.go Logs:
//! `for _, log := range logs { notifier.Notify(...) }`), квитанции — все логи блока разом (node/receipts.rs deliver).
//! Канал не пуст — логи ещё в пути (пачка отстаёт): пул ждёт их (не котируется), закроет следующая голова при пустом
//! канале. При полосах решения 330 «канал» — канал пачки плюс события полосы этого пула. Числа здесь нет — только порядок блоков и пустота канала (настройка не нужна).
//! Кто вызывает: pools/session.rs (ветка новой головы, до reload::retry).
//! От чего зависит: settle.rs (Holds), reload.rs (load_one), stale_read.rs (requeue, waiting), admit.rs (Members),
//! refresh_urgency.rs (класс dex.event_refresh_urgency), listen.rs (Pools, End).
//! Настройки: через вызываемые — dex.event_refresh_urgency, настройки чтения вида.

use crate::storage::journal::Level;

use super::admit::Members;
use super::listen::{End, Pools};
use super::settle::Hold;
use super::{refresh_urgency, reload, stale_read};

/// Пора ли закрыть отметку на голове `at` при `queued` сообщениях в канале пачки (см. шапку). Ошибок нет.
pub fn due(h: &Hold, at: u64, queued: usize) -> bool {
    h.need.is_some() && at > h.block && queued == 0
}

/// Новая голова: закрыть «в обновлении» у пулов пачки, которым пора (due). Принимает: окружение, пачку, голову,
/// сколько сообщений «в пути» у пула с индексом i (канал пачки; при полосах решения 330 — плюс события полосы пула,
/// session_lanes.rs queued). Ошибки: End — ядро закрыло канал.
pub async fn on_head(
    p: &Pools,
    m: &mut Members,
    at: u64,
    queued: impl Fn(usize) -> usize,
) -> Result<(), End> {
    if !p.holds.any() {
        return Ok(());
    }
    let Members {
        pools,
        missing,
        abandoned,
        ..
    } = m;
    for (i, (f, t)) in pools.iter().enumerate() {
        let Some(h) = p.holds.get(&f.reference) else {
            continue;
        };
        if !due(&h, at, queued(i)) {
            continue;
        }
        p.holds.forget(&f.reference);
        // Ждёт повтора (решение 298) — его прочитает reload::retry на этой же голове.
        if stale_read::waiting(missing, i) {
            continue;
        }
        let msg = format!(
            "{} {t}: логи блока {} пула не пришли целиком к голове {at} — пул перечитывается целиком на голове \
             (решение 357; до этого пул не котировался и ядру не отправлялся)",
            f.family.title, h.block
        );
        p.ctx.log(Level::Info, "dex.pool", msg, None).await;
        let res = refresh_urgency::urgent(&p.ctx, reload::load_one(p, f, t, at)).await?;
        if res.is_err() {
            stale_read::requeue(missing, abandoned, i);
        }
    }
    Ok(())
}
