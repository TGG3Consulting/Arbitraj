//! Файл: core/settlement_accounts.rs.
//! Что делает: проверяет исходный подписант и торговый счёт при восстановлении и непосредственно перед отправкой.
//! Кто вызывает: исполнитель круга и защита перед записью и отправкой.
//! От чего зависит: адаптеры исходной биржи и конкретного кошелька.
//! Настройки: маппинг направлений и текущие ключи площадок.
use super::{engine::Core, settlement_market::Market, types::DexAccountId};
use crate::gateway::contract::Venue;
use serde::{Deserialize, Serialize};
use std::{future::Future, sync::Arc};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Binding {
    wallet: String,
    cex: String,
}

#[derive(Clone)]
pub(super) struct Accounts {
    dex: Arc<dyn Venue>,
    cex: Arc<dyn Venue>,
    expected: Binding,
}
tokio::task_local! {static ACTIVE:(Accounts,Core,super::settlement_cycle::Mandate);}
tokio::task_local! {static ENTRY:();}
pub(super) async fn entry<T>(f: impl Future<Output = T>) -> T {
    ENTRY.scope((), f).await
}
impl Accounts {
    pub async fn read(core: &Core, market: &Market, wallet: &DexAccountId) -> Result<Self, String> {
        let gw = &core.inner.deps.gw;
        let dex = gw
            .get_as(&market.dex, Some(wallet))
            .await
            .map_err(|e| e.message)?;
        let cex = gw.get(&market.cex).await.map_err(|e| e.message)?;
        let expected = Binding {
            wallet: dex.settlement_account().map_err(|e| e.message)?,
            cex: cex.settlement_account().map_err(|e| e.message)?,
        };
        Ok(Self { dex, cex, expected })
    }
    pub fn binding(&self) -> Binding {
        self.expected.clone()
    }
    pub async fn run<T>(
        self,
        core: &Core,
        plan: &super::settlement_cycle::Mandate,
        f: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        if self.expected != plan.accounts {
            return Err(
                "Изменился исходный торговый счёт или подписант. Исходные ID доступны для сверки"
                    .into(),
            );
        }
        let signer = self.expected.cex.clone();
        ACTIVE
            .scope(
                (self, core.clone(), plan.clone()),
                crate::gateway::cash_signer::scope(
                    signer,
                    crate::gateway::settlement_proofs::scope(f),
                ),
            )
            .await
    }
    fn verify(&self) -> Result<(), String> {
        if self.dex.settlement_account().map_err(|e| e.message)? != self.expected.wallet
            || self.cex.settlement_account().map_err(|e| e.message)? != self.expected.cex
        {
            return Err("Реквизиты исходного счёта изменились перед отправкой".into());
        }
        Ok(())
    }
}
pub(super) fn guard() -> impl Fn() -> Result<(), String> + Send + Sync + 'static {
    let bound = ACTIVE.try_with(Clone::clone).ok();
    let entry = ENTRY.try_with(|_| ()).is_ok();
    move || {
        let (accounts, core, p) = bound
            .as_ref()
            .ok_or("Для отправки требуется сохранённая привязка исходного счёта и подписанта")?;
        accounts.verify()?;
        let d = &p.direction;
        let m = &p.market;
        if super::dex_account::of_direction_now(core, d).as_ref() != Some(&p.wallet)
            || super::venue_ticker::ticker_on(core, &d.asset, &m.cex) != p.target_cex
            || super::venue_ticker::ticker_on(core, &d.asset, &m.dex) != p.target_dex
        {
            return Err(
                "Изменились кошелёк или символ подтверждённого торгового направления".into(),
            );
        }
        if entry {
            super::risk::can_open(core, d)?;
        }
        Ok(())
    }
}
