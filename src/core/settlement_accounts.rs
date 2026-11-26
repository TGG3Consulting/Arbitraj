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
tokio::task_local! {static ACTIVE: Accounts;}
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
        expected: &Binding,
        f: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        if &self.expected != expected {
            return Err(
                "Изменился исходный торговый счёт или подписант. Исходные ID доступны для сверки"
                    .into(),
            );
        }
        ACTIVE.scope(self, f).await
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
    move || bound.as_ref().map_or(Ok(()), Accounts::verify)
}
