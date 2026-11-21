//! Файл: venues/bsc_dex/venue_account.rs
//! Что делает: вид адаптера DEX на счёт (решение 132, contract::for_account): те же пулы, котировки, проверки и
//! настройки, но ключ, очередь транзакций и nonce, разрешения, ожидаемые поступления и исполнитель — кошелька счёта;
//! исполнение через KyberSwap (exec.via_kyber) — тем же кошельком и в его очереди (решение 112). Вид на счёт не
//! подписывается на рынок (подписка — у основного адаптера).
//! Кто вызывает: venue.rs (for_account), gateway/registry.rs (get_as) — ядро и API через шлюз.
//! От чего зависит: mod.rs (BscDex), parts_wallet.rs, ctx_wallet.rs, venues/kyberswap/state.rs.
//! Настройки: нет (кошельки — экран «Маппинг кошельков»).

use std::sync::Arc;

use crate::core::types::DexAccountId;
use crate::gateway::contract::Venue;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::BscDex;
use super::ctx_wallet::SCREEN;

impl BscDex {
    /// Вид адаптера на кошелёк счёта. Принимает: id счёта. Возвращает: адаптер для шлюза.
    /// Ошибки: NotConfigured — такого кошелька нет.
    pub fn on_account(&self, account: &DexAccountId) -> GatewayResult<Arc<dyn Venue>> {
        let w = self
            .p
            .ctx
            .settings
            .accounts()
            .wallet(&account.0)
            .ok_or_else(|| {
                self.p.ctx.err(
                    ErrorCode::NotConfigured,
                    format!("Кошелька DEX {account} нет ({SCREEN})"),
                )
            })?;
        let p = self.p.bound(&w.id);
        let (lock, next) = p.nonces.shared();
        let kyber = self.kyber.for_wallet(w.secret, lock, next);
        Ok(Arc::new(BscDex {
            p,
            subs: self.subs.clone(),
            kyber,
        }))
    }

    /// Привязан ли адаптер к счёту. Ошибок нет.
    pub fn is_bound(&self) -> bool {
        self.p.ctx.account.is_some()
    }
}
