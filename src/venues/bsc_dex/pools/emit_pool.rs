//! Файл: venues/bsc_dex/pools/emit_pool.rs
//! Что делает: отправка пула ядру (MarketEvent::Pool) без ожидания — медленное ядро не задерживает чтение событий
//! (свежий пул уже в реестре). Канал ядра полон (ёмкость — аргумент запуска --live-buffer, core/engine.rs):
//! - dex.pool_events_keep не «да» (не задана) — как раньше: событие не ушло, счёт потерь (решение 313, lost_events.rs);
//! - «да» (решение 321) — событие не теряется: последнее событие пула ждёт в наборе и уходит ядру, как только в канале
//!   появится место (сначала ждущие, по очереди; новые события пулов, пока пулы этой площадки ждут, встают за ними).
//!   Решение 324: набор — ОБЩИЙ на канал ядра для пулов DEX и стаканов всех бирж, с одним доставщиком и общей очередью
//!   (gateway/core_out.rs; ключ — (площадка, пул)); находится по самому каналу (core_out::of) — Pools.kept.
//!   Перед отправкой пул проверяется по реестру (Live площадки в наборе — Feed::live): снят или недостоверен (после
//!   события «пулы недостоверны» ядро его уже убрало) — не отправляется, чтобы не вернуть ядру устаревшее. Уточнение 324: есть пути, где пул
//!   становится недостоверным без Invalidated (например, per_block.rs ставит valid = false) — ждавшее событие такого
//!   пула тоже не отправляется, и ядро остаётся с прежним состоянием пула — так же, как раньше при потере события.
//!   Запись журнала dex.pool_events_lost при «да» — «отложено N, заменено R, доставлено M, снято S, ждут K»
//!   (lost_events.rs kept; при опустевшем наборе — крючок on_drain).
//!
//! Пока пулы этой площадки ждут, их новые события встают в очередь и без чтения настройки: выключение действует с
//! момента, когда пулы площадки в наборе кончились (ждущие доставляются до конца).
//! Кто вызывает: pools/listen_log.rs (emit — реэкспорт: on_log; reload.rs, reload_many.rs, per_block_lb.rs,
//! per_block_balancer.rs зовут listen_log::emit); parts.rs (Kept::new, Kept::watch).
//! От чего зависит: listen.rs (Pools: канал, набор), depth.rs (touch), gateway/core_out.rs, gateway/core_out_item.rs,
//! lost_events.rs, registry.rs (entry — достоверность при доставке).
//! Настройки: dex.pool_events_keep [bsc_dex] (да/нет, необязательная; нужна только при полном канале, когда пулы
//! площадки не ждут; ревью 324, Б2: значение в памяти — gateway/setting_cell.rs, состояние класса Feed::side, без
//! Store::resolve на событие); через lost_events.rs — log.pool_events_lost_interval_s.

use std::sync::Arc;

use tokio::sync::mpsc::Sender;

use crate::core::types::{Pool, VenueId};
use crate::gateway::core_out::{self, Feed};
use crate::gateway::core_out_item::{Class, Item, Key, Live};
use crate::gateway::core_out_side::Side;
use crate::gateway::events::MarketEvent;
use crate::gateway::keep_last::{Ask, Fast, Offer};
use crate::gateway::setting_cell::{self, Cell, bool_of};

use super::super::ctx::Ctx;
use super::listen::{End, Pools};
use super::lost_events::Lost;
use super::registry::Registry;

/// Имя настройки решения 321.
const KEEP: &str = "dex.pool_events_keep";

/// Общий набор канала ядра, площадка и настройка в памяти (одно на окружение — без выделения памяти на событие).
/// Проверка «пул ещё годен» — в наборе, одна на площадку (Feed::live, ревью 324 Б1).
#[derive(Clone)]
pub struct Kept {
    pub feed: Feed,
    /// Площадка — готовый VenueId для ключа медленного пути (не строка на событие).
    pub venue: VenueId,
    /// dex.pool_events_keep в памяти (ревью 324, Б2).
    pub keep: Arc<Cell>,
}

impl Kept {
    /// Набор канала ядра, проверка по реестру (в набор, по площадке), настройка в памяти (одна на класс «пулы
    /// площадки» набора). Принимает: канал ядра, реестр, окружение. Ошибок нет.
    pub fn new(out: &Sender<MarketEvent>, reg: &Registry, ctx: &Ctx) -> Self {
        let reg = reg.clone();
        let live: Live = Arc::new(move |ev: &MarketEvent| match ev {
            MarketEvent::Pool(p) => reg.entry(&p.reference).is_some_and(|e| e.valid),
            _ => true,
        });
        let feed = core_out::of(out);
        let venue = ctx.venue();
        feed.live(venue.clone(), live);
        let at = ctx.at().venue.map(str::to_string);
        let (side, _) = feed.side(&Class::Pool(venue.clone()), || {
            Side::new(setting_cell::watch(&ctx.settings, KEEP, at, bool_of), None)
        });
        Self {
            feed,
            venue,
            keep: side.keep.clone(),
        }
    }

    /// Крючок «набор опустел» класса пулов площадки — запись журнала (не чаще интервала). Принимает: окружение,
    /// счётчик потерь подписки. Ошибок нет.
    pub fn watch(&self, ctx: &Ctx, lost: &Lost) {
        let (ctx2, lost) = (ctx.clone(), lost.clone());
        let hook = move |f: &Feed| lost.kept(&ctx2, f);
        self.feed.on_drain(Class::Pool(ctx.venue()), Arc::new(hook));
    }
}

/// Пул — ядру без ожидания (см. шапку). Принимает: окружение пачки, пул. Ошибки: End::Closed — ядро закрыло канал.
pub fn emit(p: &Pools, pool: Pool) -> Result<(), End> {
    super::depth::touch(p, &pool.reference);
    let k = &p.kept;
    let r = match k.feed.fast(&p.out) {
        // Набор пуст (ревью 324, Б1): одна атомарная загрузка + try_reserve + send — без замка, ключа, настройки.
        Fast::Free(permit) => {
            permit.send(MarketEvent::Pool(pool));
            return Ok(());
        }
        Fast::Closed => Offer::Closed,
        Fast::Full if !k.keep.on() => Offer::Lost,
        Fast::Full | Fast::Busy => {
            // Медленный путь: ключ строится только здесь; настройка — из памяти, прочитана вне замка набора.
            let key = Key::Pool(k.venue.clone(), pool.reference.clone());
            let on = k.keep.on();
            let item = Item::Event(MarketEvent::Pool(pool));
            k.feed.slow(&p.out, key, item, &mut Ask::new(move || on))
        }
    };
    match r {
        Offer::Sent | Offer::Replaced { .. } => Ok(()),
        Offer::Closed => Err(End::Closed),
        // Решение 313: канал ядра полон — событие не ушло; счёт и запись не чаще интервала (lost_events.rs).
        Offer::Lost => {
            p.signals.lost.note(&p.ctx);
            Ok(())
        }
        // Доставщика запускает сам набор (core_out.rs).
        Offer::Deferred { .. } => {
            p.signals.lost.kept(&p.ctx, &p.kept.feed);
            Ok(())
        }
    }
}
