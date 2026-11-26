//! Файл: venues/bsc_dex/ctx_chain.rs
//! Что делает: ревью t235 (п. 4, решение 461) — chainId сети площадки DEX окружения (dex.evm_chain_id) для горячего
//! пути (pool_gate::decide на каждый шаг пула менеджера котировки): чтение — одна загрузка OnceLock, сравнение
//! хранилища (Store::same, Arc::ptr_eq) и одна атомарная загрузка ячейки, без Store::resolve (строки областей) и без
//! замков. Механизм — тот же, что у ячеек настроек вида DEX (pools/event_flags.rs, ревью 325) и выхода шлюза
//! (gateway/setting_cell.rs watch): ячейка создаётся при первом вопросе (main.rs спрашивает при старте каждого
//! окружения площадки DEX), значение перечитывается задачей tokio по рассылке изменений хранилища — смена
//! dex.evm_chain_id в интерфейсе подхватывается на лету, без перезапуска.
//! Ячейка — поле Ctx.evm_chain (Arc): копии окружения одной площадки (with_account, clone) делят её; окружение другой
//! площадки (Ctx::for_place) получает новую. Ячейка привязана к хранилищу, с которым создана: спросили с другим
//! хранилищем (снимок решения 239) или без рантайма tokio — прямое чтение настройки, как раньше (ctx.int).
//! Кто вызывает: exec/executor/pool_gate.rs (decide), exec/executor/live_proof.rs (chain_of), main.rs (при старте).
//! От чего зависит: gateway/setting_cell.rs (watch, Cell, u64_of), settings/store.rs (same), ctx.rs (Ctx, int).
//! Настройки: dex.evm_chain_id [площадка DEX] (читается, новых настроек нет).

use std::sync::{Arc, OnceLock};

use crate::gateway::setting_cell::{self, Cell, u64_of};
use crate::settings::store::Store;

use super::ctx::Ctx;

/// Имя настройки сети площадки DEX (решение 461: ключ записи приёмки — пара версия × сеть).
pub const NAME: &str = "dex.evm_chain_id";

/// Ячейка и хранилище, с которым она создана.
struct Slot {
    store: Store,
    cell: Arc<Cell>,
}

/// Ячейка chainId площадки (поле Ctx.evm_chain). Пустая — создаётся при первом вопросе. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct ChainCell(Arc<Local>);

#[derive(Default)]
struct Local {
    id: OnceLock<Slot>,
    ring: super::pools::ring_acceptance::Book,
}
impl ChainCell {
    pub(super) fn ring_proofs(&self) -> &super::pools::ring_acceptance::Book {
        &self.0.ring
    }
}

impl Ctx {
    /// chainId сети площадки DEX окружения (dex.evm_chain_id). Горячий путь: без строк и замков (см. шапку).
    /// Возвращает: None — не задана (или не целое). Ошибок нет.
    pub fn chain_id(&self) -> Option<u64> {
        if let Some(s) = self.evm_chain.0.id.get() {
            return self.chain_from(s);
        }
        // Ячейку некому обновлять без рантайма — прямое чтение, ячейка не создаётся.
        if tokio::runtime::Handle::try_current().is_err() {
            return self.int(NAME).ok();
        }
        let s = self.evm_chain.0.id.get_or_init(|| Slot {
            store: self.settings.clone(),
            cell: setting_cell::watch(
                &self.settings,
                NAME,
                Some(self.place.id.to_string()),
                u64_of,
            ),
        });
        self.chain_from(s)
    }

    /// Значение ячейки, если она того же хранилища; иначе прямое чтение. Ошибок нет.
    fn chain_from(&self, s: &Slot) -> Option<u64> {
        if s.store.same(&self.settings) {
            s.cell.get()
        } else {
            self.int(NAME).ok()
        }
    }
}
