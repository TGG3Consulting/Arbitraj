//! Файл: core/wallet_pause.rs
//! Что делает: пауза КОШЕЛЬКА DEX (решение 230, Тигран 08.11.2026). До решения расхождение сверки одного кошелька
//! ставило паузу всей площадке DEX (bsc_dex, решение 221) — останавливались все кошельки. Теперь сверка ставит паузу
//! только этому кошельку (w1..w7): Risk.wallet_paused (id → WalletPause). Действует так:
//! - новые направления и сделки, которые пойдут этим кошельком (маппинг биржи → кошелёк, решение 132; DEX ↔ DEX —
//!   кошелёк строки «DEX ↔ DEX»), не начинаются: risk::can_open (предпроверка сделки, циклы DEX ↔ DEX) — отказ с
//!   причиной; остальные кошельки торгуют; идущие сделки доводятся до конца;
//! - перегон (relay.rs): DEX, куда монета пошла бы этим кошельком, — не цель; монета на кошельке на паузе — перегона с
//!   него нет, продажа на месте (как у паузы площадки, решение 220 В);
//! - снятие: сама — reconcile_wallet.rs (ledger.reconcile_confirm_rounds чистых кругов этого кошелька, как 221б);
//!   вручную — «Снять паузу» кошелька (POST /api/control/wallet/{id}/resume → Core::resume_wallet), журнал
//!   core.control с именем кошелька.
//!
//! Кто вызывает: risk.rs (can_open), relay.rs, reconcile_wallet.rs, api/core_api.rs (resume_wallet), commands_control.rs
//! (resume_wallet — кнопка Telegram «Снять паузу кошелька», ManualAction::ResumeWallet).
//! От чего зависит: engine.rs, risk.rs, dex_account.rs, trade_ops.rs (вид площадки), deal.rs.
//! Настройки: нет.

use serde::Serialize;

use crate::core::types::DexAccountId;
use crate::gateway::contract::VenueKind;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::{Core, lk};

/// Пауза кошелька: имя (на момент постановки), площадка DEX, причина.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WalletPause {
    pub name: String,
    pub venue: String,
    pub why: String,
}

impl WalletPause {
    /// Подпись кошелька для человека: «кошелёк «имя»». Ошибок нет.
    pub fn who(&self) -> String {
        format!("кошелёк «{}»", self.name)
    }

    /// Причина отказа новой сделки этим кошельком. Ошибок нет.
    pub fn refusal(&self) -> String {
        format!("{} ({}) на паузе: {}", self.who(), self.venue, self.why)
    }
}

/// Пауза кошелька, если стоит. Принимает: ядро, счёт (None — не DEX). Возвращает: копию паузы. Ошибок нет.
pub fn paused(core: &Core, acc: Option<&DexAccountId>) -> Option<WalletPause> {
    let acc = acc?;
    lk(&core.inner.risk).wallet_paused.get(&acc.0).cloned()
}

/// Монета сделки лежит на кошельке DEX на паузе (relay.rs: перегона с него нет). Принимает: ядро, сделку.
/// Возвращает: пауза кошелька или None (площадка сделки не DEX, счёта нет, паузы нет). Ошибок нет.
pub fn paused_at(core: &Core, deal: &Deal) -> Option<WalletPause> {
    if super::trade_ops::kind(core, &deal.at).ok()? != VenueKind::Dex {
        return None;
    }
    let acc = super::dex_account::of(core, deal).ok().flatten();
    paused(core, acc.as_ref())
}

impl Core {
    /// «Снять паузу» кошелька (интерфейс). Принимает: id кошелька. Возвращает: true — пауза стояла и снята; журнал
    /// core.control с именем кошелька. Ошибок нет.
    pub async fn resume_wallet(&self, id: &str) -> bool {
        let p = lk(&self.inner.risk).wallet_paused.remove(id);
        let Some(p) = p else {
            return false;
        };
        super::risk::publish(self);
        let text = format!("Пауза {} снята вручную (была: {})", p.who(), p.why);
        self.inner
            .deps
            .log(Level::Info, "core.control", Some(&p.venue), &text, None)
            .await;
        true
    }
}
