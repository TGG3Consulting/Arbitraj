//! Файл: venues/bsc_dex/resubscribe_wallets.rs
//! Что делает: задачи кошельков DEX при подписке на рынок (решение 132): у каждого кошелька — своя очередь транзакций
//! (после запуска — из SQLite; остановленная — сразу ядру событием WalletQueue с id счёта) и свои изменения баланса
//! событиями (wallet/events.rs, решение 76) — для зачисления переводов без опроса. Кошельки добавили, удалили или
//! изменили — задачи кошельков пересобираются, подписка на пулы не трогается.
//! Кто вызывает: resubscribe.rs (subscribe, watch).
//! От чего зависит: parts.rs, parts_wallet.rs, wallet/{nonce,events}.rs, settings/accounts/.
//! Решение 333 (ревью Б1): событие WalletQueue — gateway/core_out_must.rs: при полном канале ядра не теряется никогда.
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
                    state: st.clone(),
                    nonce_taken: None,
                    note: None,
                };
                // Решение 333 (ревью Б1): «остановлена» не теряется при полном канале ядра — ждёт места в очереди
                // денежных событий (впереди уведомлений), без настройки.
                MustOut::new(pools.out.clone(), &p.ctx.settings, &p.ctx.journal)
                    .send(MarketEvent::WalletQueue(q));
                // Решение 408 (ответ Тиграна 9б): остановленная до перезапуска — тоже проверка по блокам (одна задача
                // на транзакцию, wallet/auto_release.rs); nonce и хеш неизвестны — только человек.
                // Ключ закрыт (секреты не разблокированы) — адрес возьмёт сама задача после разблокировки (ревью t193 М5).
                if let WalletQueueState::Halted {
                    nonce: Some(n),
                    hash: Some(h),
                    ..
                } = &st
                {
                    super::wallet::auto_release::spawn(super::wallet::auto_release::Watch {
                        ctx: b.ctx.clone(),
                        heads: b.heads.clone(),
                        stats: b.stats.clone(),
                        queue: b.nonces.queue_ref(),
                        from: super::wallet::signer::address(&b.ctx).ok(),
                        nonce: *n,
                        hash: *h,
                    });
                }
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
