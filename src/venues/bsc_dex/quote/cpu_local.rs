//! Файл: venues/bsc_dex/quote/cpu_local.rs
//! Что делает: решение 336 (задача t157) — признаки потока счёта котировки DEX (поток блокирующих задач, quote/cpu.rs):
//! ярус счёта (gateway/cpu_budget.rs Tier — вспомогательные потоки quote/par.rs берут места этого яруса) и короб
//! подбора объёма ядра (gateway/calc_share.rs — допуск путей графа один раз на подбор, quote/graph_reuse.rs).
//! Признаки задачи tokio (task_local) в поток блокирующих задач сами не переходят, поэтому cpu::run ставит их здесь —
//! в поле потока (thread_local) на время одного счёта (enter) и снимает после (Drop стража, в том числе при панике).
//! Вне счёта (тесты, фоновый граф) — ярус «фон», короба нет; тест может поставить их сам (enter).
//! Кто вызывает: quote/cpu.rs (enter), quote/par.rs (tier), quote/graph_reuse.rs (share), тесты.
//! От чего зависит: gateway/cpu_budget.rs, gateway/calc_share.rs, std (thread_local).
//! Настройки: нет.

use std::cell::RefCell;
use std::sync::Arc;

use crate::gateway::calc_share::Share;
use crate::gateway::cpu_budget::Tier;

/// Признаки одного счёта.
#[derive(Clone)]
struct Local {
    tier: Tier,
    share: Option<Arc<Share>>,
}

thread_local! {
    static LOCAL: RefCell<Option<Local>> = const { RefCell::new(None) };
}

/// Страж: возвращает прежние признаки потока при выходе из счёта.
pub struct Enter(Option<Local>);

impl Drop for Enter {
    fn drop(&mut self) {
        let prev = self.0.take();
        LOCAL.with(|l| *l.borrow_mut() = prev);
    }
}

/// Ставит признаки счёта на текущий поток. Принимает: ярус, короб подбора (None — вне подбора). Возвращает: страж
/// (держать до конца счёта). Ошибок нет.
pub fn enter(tier: Tier, share: Option<Arc<Share>>) -> Enter {
    let prev = LOCAL.with(|l| l.borrow_mut().replace(Local { tier, share }));
    Enter(prev)
}

/// Ярус текущего счёта; вне счёта — фон. Ошибок нет.
pub fn tier() -> Tier {
    LOCAL.with(|l| l.borrow().as_ref().map_or(Tier::Background, |x| x.tier))
}

/// Короб подбора текущего счёта; None — вне подбора или вне счёта. Ошибок нет.
pub fn share() -> Option<Arc<Share>> {
    LOCAL.with(|l| l.borrow().as_ref().and_then(|x| x.share.clone()))
}
