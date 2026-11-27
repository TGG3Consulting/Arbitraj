//! Отрицательное наблюдение действительно только при тех же кодах и состоянии
//! аккаунтов, участвовавших в обоих balanceOf. Пустой массив EIP-1186 запрашивает
//! исключительно корень состояния/код аккаунта. Ключи его хранилища не запрашиваются.
use super::{observed::Access, *};
use serde::Deserialize;
use std::collections::BTreeSet;

pub(in crate::venues::bsc_dex::exec) enum Denial {
    Temporary(String),
    Observed { why: String, accounts: Vec<Address> },
}
impl From<String> for Denial {
    fn from(why: String) -> Self {
        Self::Temporary(why)
    }
}
impl From<&str> for Denial {
    fn from(why: &str) -> Self {
        Self::Temporary(why.into())
    }
}
impl Denial {
    pub(super) fn observed(why: String, a: &Access, b: &Access, token: Address) -> Self {
        if !a.completed() || !b.completed() {
            return Self::Temporary(why);
        }
        let accounts: BTreeSet<_> = a.accounts().chain(b.accounts()).chain([token]).collect();
        // Проверяемая форма поддерживает токен и один дополнительный аккаунт.
        // Иной набор не порождает дополнительных фоновых обращений по всем его адресам.
        if accounts.len() > 2 {
            return Self::Temporary(why);
        }
        Self::Observed {
            why,
            accounts: accounts.into_iter().collect(),
        }
    }

    pub(in crate::venues::bsc_dex::exec) async fn record(
        self,
        ctx: &Ctx,
        generation: u64,
        h: Head,
        cache: &Cache,
    ) -> (String, Option<Negative>) {
        let (why, accounts) = match self {
            Self::Temporary(why) => return (why, None),
            Self::Observed { why, accounts } => (why, accounts),
        };
        let mut states = Vec::new();
        for address in accounts {
            match cache.account(ctx, address, generation, h).await {
                Ok(state) => states.push(state),
                Err(e) => return (format!("{why}; снимок отказа недоступен: {e}"), None),
            }
        }
        (why.clone(), Some(Negative { why, states }))
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(in crate::venues::bsc_dex::exec) struct Account {
    pub(in crate::venues::bsc_dex::exec) address: Address,
    code_hash: B256,
    storage_hash: B256,
    balance: U256,
}

#[derive(Clone)]
pub(in crate::venues::bsc_dex::exec) struct Negative {
    pub(in crate::venues::bsc_dex::exec) why: String,
    states: Vec<Account>,
}
impl Negative {
    pub(in crate::venues::bsc_dex::exec) async fn unchanged(
        &self,
        ctx: &Ctx,
        generation: u64,
        h: Head,
        cache: &Cache,
    ) -> Result<bool, String> {
        for expected in &self.states {
            if cache.account(ctx, expected.address, generation, h).await? != *expected {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
