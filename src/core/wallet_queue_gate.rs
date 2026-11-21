//! Файл: core/wallet_queue_gate.rs
//! Что делает: решение 231 (дополнение, п. 4; Тигран 08.11: «ок») — сделка «биржа → DEX» не начинается кошельком, чья
//! очередь транзакций остановлена до выяснения (WalletQueueState::Halted: исход транзакции неизвестен, узел не
//! прочитал nonce и т. п. — venues/bsc_dex/send/settle.rs). Иначе бот купит монету на бирже и переведёт её на кошелёк,
//! а продажа на DEX упрётся в остановленную очередь (QueueHalted). Сделки «DEX → биржа» и циклы DEX ↔ DEX упираются в
//! очередь на первом шаге, ничего не потратив, — их не трогаем; биржа ↔ биржа очередь кошелька не использует.
//! Состояние очереди — из рынка ядра (MarketState.queues по событию MarketEvent::WalletQueue; остановленная после
//! запуска — тоже событием, venues/bsc_dex/resubscribe_wallets.rs).
//! Кто вызывает: risk.rs (can_open).
//! От чего зависит: engine.rs (рынок), trade_ops.rs (вид площадки), core/types.
//! Настройки: нет.

use crate::core::types::{DexAccountId, Direction, WalletQueueState};
use crate::gateway::contract::VenueKind;

use super::engine::{Core, rd};

/// Почему направление «биржа → DEX» не начинается из-за остановленной очереди кошелька. Принимает: ядро, направление,
/// кошелёк направления по маппингу (dex_account::of_direction_now; None — без счёта). Возвращает: None — можно;
/// Some(причина) — очередь кошелька DEX-стороны остановлена. Ошибок нет: вид площадки не известен — не мешаем
/// (проверки готовности — risk_ready.rs).
pub fn halted_for(core: &Core, d: &Direction, wallet: Option<&DexAccountId>) -> Option<String> {
    let kind = |v| super::trade_ops::kind(core, v).ok();
    if kind(&d.buy_on) != Some(VenueKind::Cex) || kind(&d.sell_on) != Some(VenueKind::Dex) {
        return None;
    }
    let m = rd(&core.inner.market);
    match m.queues.get(&(d.sell_on.clone(), wallet.cloned())) {
        Some(WalletQueueState::Halted { reason, .. }) => Some(format!(
            "очередь транзакций кошелька {} на {} остановлена ({reason}) — сделка биржа → DEX не начинается: продажа на DEX не ушла бы",
            wallet.map_or("без счёта", |w| w.0.as_str()),
            d.sell_on.0
        )),
        _ => None,
    }
}
