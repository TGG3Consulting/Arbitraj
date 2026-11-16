//! Файл: venues/bsc_dex/resubscribe_wallets.rs
//! Что делает: задачи кошельков DEX при подписке на рынок (решение 132): у каждого кошелька — своя очередь транзакций
//! (после запуска — из SQLite; остановленная — сразу ядру событием WalletQueue с id счёта) и свои изменения баланса
//! событиями (wallet/events.rs, решение 76) — для зачисления переводов без опроса. Кошельки добавили, удалили или
//! изменили — задачи кошельков пересобираются, подписка на пулы не трогается.
//! Кто вызывает: resubscribe.rs (subscribe, watch).
//! От чего зависит: parts.rs, parts_wallet.rs, wallet/{nonce,events}.rs, settings/accounts/.
//! Решение 333: событие WalletQueue — gateway/core_out_must.rs (venue.money_events_keep, log.events_lost_interval_s).
//! Настройки: нет своих (события — venue.balance_events и др., wallet/events.rs).

use crate::core::types::{DexAccountId, WalletQueue, WalletQueueState};
use crate::gateway::core_out_must::MustOut;
use crate::gateway::events::MarketEvent;

use super::parts::Parts;
use super::pools::listen::Pools;
use super::resubscribe::Subs;

impl Subs {
    /// Перезапускает задачи кошельков. Принимает: состояние адаптера, окружение слушателей (новая подписка) или None —
    /// прежнее (изменились кошельки). Ошибок нет: подписки ещё не было — ничего не делает.
    pub async fn wallets(&self, p: &Parts, pools: Option<Pools>) {
        let pools = {
            let mut cur = self.pools.lock().unwrap_or_else(|x| x.into_inner());
            if let Some(x) = pools {
                *cur = Some(x);
            }
            cur.clone()
        };
        let Some(pools) = pools else { return };
        let mut new = Vec::new();
        for w in p.ctx.settings.accounts().wallets() {
            let b = p.bound(&w.id);
            b.nonces.restore(&b.ctx).await;
            if let st @ WalletQueueState::Halted { .. } = b.nonces.state() {
                let q = WalletQueue {
                    venue: b.ctx.venue(),
                    account: Some(DexAccountId(w.id.clone())),
                    state: st,
                    nonce_taken: None,
                };
                // Решение 333: «остановлена» не теряется молча при полном канале ядра (venue.money_events_keep = да —
                // ждёт места по порядку; иначе потеря со счётом в журнале venue.core_events_lost).
                MustOut::new(pools.out.clone(), &p.ctx.settings, &p.ctx.journal)
                    .send(MarketEvent::WalletQueue(q));
            }
            let mut pw = pools.clone();
            pw.ctx = b.ctx.clone();
            new.push(tokio::spawn(super::wallet::events::run(
                pw,
                b.inflows.clone(),
            )));
        }
        let mut tasks = self.wallet_tasks.lock().unwrap_or_else(|x| x.into_inner());
        for old in tasks.drain(..) {
            old.abort();
        }
        *tasks = new;
    }
}
