//! Файл: venues/bsc_dex/pools/settle_close.rs
//! Что делает: решение 357 — предел «в обновлении» (settle.rs Need::Active / Follow): недостающие логи блока пула так
//! и не пришли, а сеть ушла дальше — пул читается ЦЕЛИКОМ на голове (reload::load_one: состояние на конце блока головы,
//! valid = true; поздние логи блоков до головы отбрасываются по has_applied) и уходит ядру. Не прочитался —
//! недостоверен, Invalidated по монете, причина и пауза решения 339 (pool_fail::note) и очередь повтора пачки
//! (pool_fail::requeue: брошенный из-за незаданной настройки при 339 так и остаётся брошенным — ревью слияния, Н2).
//! Когда закрывать (ревью слияния, Н3): новая голова `at` (node/heads.rs; сессия пачки — session.rs) ВЫШЕ блока
//! отметки, и логам ЭТОГО пула негде ждать: в его полосе (решение 330) ничего нет, и либо канал пачки пуст, либо
//! сессия уже приняла из канала лог глубокого пула более позднего блока, чем блок отметки (`seen`). Почему второе
//! достаточно: подписка «все события» глубоких пулов пачки — один поток уведомлений, упорядоченный по (блок, logIndex)
//! (geth шлёт логи блока подряд, по одному уведомлению — bnb-chain/bsc eth/filters/api.go Logs; квитанции — все логи
//! блока разом, node/receipts.rs deliver), значит, приняв лог блока > B, сессия уже приняла все логи блока B этой
//! подписки — недостающим негде быть, кроме полосы пула. Оговорка: на стыке смены состава подписки (group.rs set: новая
//! подписка, затем отписка старой) два потока «все события» идут одновременно — закрытие может случиться раньше; это
//! полное чтение пула на голове (не возврат достоверности), лишняя работа, не деньги. Числа и настройки нет.
//! Решение 382: так же закрывается отметка Need::Reread (память пула разошлась с узлом) — полное чтение на голове выше
//! блока отметки.
//! Кто вызывает: pools/session.rs (ветка новой головы, до reload::retry).
//! От чего зависит: settle.rs (Holds, Hold), reload.rs (load_one), stale_read.rs (waiting), pool_fail.rs (note, requeue,
//! base), invalidated.rs, admit.rs (Members), refresh_urgency.rs (класс dex.event_refresh_urgency), listen.rs.
//! Настройки: через вызываемые — dex.event_refresh_urgency, dex.pool_fail_pause_ms, настройки чтения вида.

use crate::storage::journal::Level;

use super::admit::Members;
use super::listen::{End, Pools};
use super::settle::{Hold, Need};
use super::{pool_fail, refresh_urgency, reload, stale_read};

/// Пора ли закрыть отметку на голове `at`; `transit` — могут ли логи пула блока отметки ещё быть в пути (см. шапку).
/// Ошибок нет.
pub fn due(h: &Hold, at: u64, transit: bool) -> bool {
    h.need.is_some() && at > h.block && !transit
}

/// В пути ли логи пула (см. шапку). Принимает: событий в полосе пула, сообщений в канале пачки, самый поздний блок
/// принятых логов глубоких пулов, блок отметки. Ошибок нет.
pub fn in_transit(lane: usize, chan: usize, seen: u64, block: u64) -> bool {
    lane > 0 || (chan > 0 && seen <= block)
}

/// Новая голова: закрыть «в обновлении» у пулов пачки, которым пора (due). Принимает: окружение, пачку, голову,
/// функцию «в пути ли логи пула i для блока отметки». Ошибки: End — ядро закрыло канал.
pub async fn on_head(
    p: &Pools,
    m: &mut Members,
    at: u64,
    transit: impl Fn(usize, u64) -> bool,
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
        if !due(&h, at, transit(i, h.block)) {
            continue;
        }
        p.holds.forget(&f.reference);
        // Ждёт повтора (решение 298) — его прочитает reload::retry на этой же голове.
        if stale_read::waiting(missing, i) {
            continue;
        }
        let msg = if h.need == Some(Need::Reread) {
            format!(
                "{} {t}: память пула разошлась с узлом на блоке {} — пул перечитывается целиком на голове {at} \
                 (решение 382; до этого пул не котировался и ядру не отправлялся)",
                f.family.title, h.block
            )
        } else {
            format!(
                "{} {t}: логи блока {} пула не пришли целиком к голове {at} — пул перечитывается целиком на голове \
                 (решение 357; до этого пул не котировался и ядру не отправлялся)",
                f.family.title, h.block
            )
        };
        p.ctx.log(Level::Info, "dex.pool", msg, None).await;
        let res = refresh_urgency::urgent(&p.ctx, reload::load_one(p, f, t, at)).await?;
        if let Err(why) = res {
            // Ревью слияния (Н2): как ошибка чтения по событию — Invalidated, причина и пауза 339, повтор.
            p.reg.update(&f.reference, |e| e.valid = false);
            super::invalidated::send(p, f.coin, t.clone());
            pool_fail::note(p, &f.reference, f.family.title, t, &why).await;
            pool_fail::requeue(pool_fail::base(&p.ctx).is_some(), missing, abandoned, i);
        }
    }
    Ok(())
}
