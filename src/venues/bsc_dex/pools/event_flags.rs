//! Файл: venues/bsc_dex/pools/event_flags.rs
//! Что делает: ревью 325 (Н1) — настройки решения 325 по одному DEX (область «DEX») для пути события пула без чтения
//! хранилища на каждое событие: dex.event_refresh_batch (перечитывание по событию с ответами, прочитанными заранее —
//! prefetch_read.rs; бины Infinity Bin пачкой — reread_bins.rs) и dex.event_refresh_fee_only (только комиссия Algebra
//! Integral — integral_fee.rs). Значение — в ячейке памяти (gateway/setting_cell.rs watch_in, как dex.pool_events_keep
//! решения 324): чтение — одна загрузка OnceLock и одна атомарная загрузка ячейки, без строки, без замка хранилища.
//! Ячейка создаётся при первом вопросе по этому DEX (одно чтение хранилища) и перечитывается задачей tokio по
//! рассылке изменений хранилища — смена настройки в интерфейсе подхватывается на лету, без перезапуска (задача — одна
//! на DEX и настройку, живёт весь процесс, как и вид DEX — families.rs).
//! Ячейка привязана к хранилищу, с которым создана (Store::same): спросили с другим хранилищем (снимок решения 239,
//! тесты со своей базой) — чтение прямо из хранилища, как до ревью. Нет рантайма tokio — тоже прямое чтение (ячейку
//! некому обновлять).
//! Кто вызывает: pools/prefetch_read.rs (size), pools/reread_bins.rs, pools/integral_fee.rs (fits).
//! От чего зависит: gateway/setting_cell.rs (watch_in, Cell, bool_of), settings/scope.rs (Scope), settings/store.rs
//! (same), ctx.rs (dex_flag — прямое чтение), families.rs (Family — ячейки живут в виде DEX).
//! Настройки: dex.event_refresh_batch [DEX], dex.event_refresh_fee_only [DEX].

use std::fmt;
use std::sync::{Arc, OnceLock};

use crate::gateway::setting_cell::{self, Cell, bool_of};
use crate::settings::scope::Scope;
use crate::settings::store::Store;

use super::super::ctx::Ctx;
use super::families::Family;

/// Имя настройки «перечитывание по событию с ответами, прочитанными заранее» (решение 325).
pub const BATCH: &str = "dex.event_refresh_batch";
/// Имя настройки «у Algebra Integral — только комиссия» (решение 325).
pub const FEE_ONLY: &str = "dex.event_refresh_fee_only";

/// Ячейка одной настройки и хранилище, с которым она создана.
struct Slot {
    store: Store,
    cell: Arc<Cell>,
}

/// Ячейки настроек решения 325 одного DEX (поле вида DEX — families.rs Family::event).
#[derive(Default)]
pub struct EventFlags {
    batch: OnceLock<Slot>,
    fee_only: OnceLock<Slot>,
}

impl fmt::Debug for EventFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = |s: &OnceLock<Slot>| s.get().map(|x| x.cell.get());
        f.debug_struct("EventFlags")
            .field("batch", &v(&self.batch))
            .field("fee_only", &v(&self.fee_only))
            .finish()
    }
}

impl EventFlags {
    /// Пустые ячейки (создаются при первом вопросе). Ошибок нет.
    pub const fn new() -> EventFlags {
        EventFlags {
            batch: OnceLock::new(),
            fee_only: OnceLock::new(),
        }
    }
}

/// dex.event_refresh_batch = да для DEX вида. Не задана / нет — false. Ошибок нет.
pub fn batch(ctx: &Ctx, f: &Family) -> bool {
    on(ctx, f, &f.event.batch, BATCH)
}

/// dex.event_refresh_fee_only = да для DEX вида. Не задана / нет — false. Ошибок нет.
pub fn fee_only(ctx: &Ctx, f: &Family) -> bool {
    on(ctx, f, &f.event.fee_only, FEE_ONLY)
}

/// Значение да/нет из ячейки (см. шапку). Ошибок нет.
fn on(ctx: &Ctx, f: &Family, slot: &OnceLock<Slot>, name: &'static str) -> bool {
    if let Some(s) = slot.get() {
        if s.store.same(&ctx.settings) {
            return s.cell.on();
        }
        return direct(ctx, f, name);
    }
    // Ячейку некому обновлять без рантайма — прямое чтение, ячейка не создаётся.
    if tokio::runtime::Handle::try_current().is_err() {
        return direct(ctx, f, name);
    }
    let s = slot.get_or_init(|| Slot {
        store: ctx.settings.clone(),
        cell: setting_cell::watch_in(&ctx.settings, name, Scope::Dex(f.id.to_string()), bool_of),
    });
    if s.store.same(&ctx.settings) {
        s.cell.on()
    } else {
        direct(ctx, f, name)
    }
}

/// Прямое чтение из хранилища (как до ревью 325). Ошибок нет: не задана — false.
fn direct(ctx: &Ctx, f: &Family, name: &str) -> bool {
    ctx.dex_flag(name, f.id).unwrap_or(false)
}
