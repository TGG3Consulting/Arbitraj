//! Файл: venues/bsc_dex/pools/emit_pool.rs
//! Что делает: отправка пула ядру (MarketEvent::Pool) без ожидания — медленное ядро не задерживает чтение событий
//! (свежий пул уже в реестре). Канал ядра полон (ёмкость — аргумент запуска --live-buffer, core/engine.rs):
//! - dex.pool_events_keep не «да» (не задана) — как раньше: событие не ушло, счёт потерь (решение 313, lost_events.rs);
//! - «да» (решение 321) — событие не теряется: последнее событие пула ждёт в наборе (pending_events.rs, общий на
//!   адаптер — Parts.kept, все подписки и пачки), задача-доставщик отдаёт ждущих ядру, как только в канале появится
//!   место (сначала ждущие, по очереди; новые события, пока кто-то ждёт, встают за ними). Перед отправкой пул
//!   проверяется по реестру: снят или недостоверен (после события «пулы недостоверны» ядро его уже убрало) — не
//!   отправляется, чтобы не вернуть ядру устаревшее. Запись журнала dex.pool_events_lost при «да» — «отложено N,
//!   заменено R, доставлено M, снято S, ждут K» (lost_events.rs kept).
//!
//! Пока кто-то ждёт, новые события встают в очередь и без чтения настройки: выключение действует с момента, когда
//! набор опустеет (ждущие доставляются до конца).
//! Кто вызывает: pools/listen_log.rs (emit — реэкспорт: on_log; reload.rs, reload_many.rs, per_block_lb.rs,
//! per_block_balancer.rs зовут listen_log::emit).
//! От чего зависит: listen.rs (Pools: канал, реестр, набор), depth.rs (touch), pending_events.rs, lost_events.rs,
//! registry.rs (entry — достоверность при доставке).
//! Настройки: dex.pool_events_keep [bsc_dex] (да/нет, необязательная; читается только при полном канале и пустом
//! наборе — Store::resolve, без текста ошибки); через lost_events.rs — log.pool_events_lost_interval_s.

use crate::core::types::{Pool, PoolRef};
use crate::gateway::events::MarketEvent;

use super::listen::{End, Pools};
use super::pending_events::Offer;

/// Имя настройки решения 321.
const KEEP: &str = "dex.pool_events_keep";

/// Пул — ядру без ожидания (см. шапку). Принимает: окружение пачки, пул. Ошибки: End::Closed — ядро закрыло канал.
pub fn emit(p: &Pools, pool: Pool) -> Result<(), End> {
    super::depth::touch(p, &pool.reference);
    let key = pool.reference.clone();
    let keep = || keep_on(p);
    match p.kept.offer(&p.out, key, pool, MarketEvent::Pool, keep) {
        Offer::Sent | Offer::Replaced => Ok(()),
        Offer::Closed => Err(End::Closed),
        // Решение 313: канал ядра полон — событие не ушло; счёт и запись не чаще интервала (lost_events.rs).
        Offer::Lost => {
            p.signals.lost.note(&p.ctx);
            Ok(())
        }
        Offer::Deferred { start } => {
            if start {
                spawn_deliver(p);
            }
            p.signals.lost.kept(&p.ctx, &p.kept);
            Ok(())
        }
    }
}

/// dex.pool_events_keep = да (не задана / нет / не да-нет — нет). Ошибок нет.
fn keep_on(p: &Pools) -> bool {
    p.ctx
        .settings
        .resolve(KEEP, p.ctx.at().venue, None)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Запускает доставщика набора (одна задача на набор; завершается, когда набор пуст или канал закрыт). По завершении
/// с пустым набором — запись журнала (не чаще интервала). Ошибок нет.
fn spawn_deliver(p: &Pools) {
    let (kept, out, reg) = (p.kept.clone(), p.out.clone(), p.reg.clone());
    let (ctx, lost) = (p.ctx.clone(), p.signals.lost.clone());
    tokio::spawn(async move {
        let live = |r: &PoolRef| reg.entry(r).is_some_and(|e| e.valid);
        if kept.clone().deliver(out, MarketEvent::Pool, live).await {
            lost.kept(&ctx, &kept);
        }
    });
}
