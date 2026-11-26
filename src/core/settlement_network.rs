//! Файл: core/settlement_network.rs.
//! Что делает: сверяет контракт и сторону перевода со свежим справочником биржи до отправки.
//! Кто вызывает: модель и денежные операции расчётного маршрута.
//! От чего зависит: сохранённый рынок и состояние ядра.
//! Настройки: сети, соответствия монет и включатели площадок.
use super::{
    engine::{Core, rd},
    settlement_market::Market,
    types::{Asset, CoinNetworkInfo},
};
impl Market {
    pub fn guard(
        &self,
        core: &Core,
        token: &Asset,
        withdraw: bool,
    ) -> impl Fn(&crate::gateway::pending::PendingRef) -> crate::gateway::errors::GatewayResult<()>
    + Send
    + Sync
    + 'static {
        let c = core.clone();
        let m = self.clone();
        let token = token.clone();
        let accounts = super::settlement_accounts::guard();
        move |_| {
            let verify = || -> Result<(), String> {
                accounts()?;
                super::executor::buy_gate(&c)?;
                if !super::registry_sync::enabled(&c, &m.dex)
                    || !super::registry_sync::enabled(&c, &m.cex)
                {
                    return Err("Одна из площадок расчётного маршрута выключена".into());
                }
                m.token_network(&c, &token, withdraw).map(|_| ())
            };
            verify().map_err(|e| {
                crate::gateway::errors::GatewayError::new(
                    crate::gateway::errors::ErrorCode::NotConfigured,
                    Some(m.dex.clone()),
                    e,
                )
            })
        }
    }
    pub(super) fn transfer_network(
        &self,
        core: &Core,
        asset: &Asset,
        venue: &super::types::VenueId,
    ) -> Result<String, String> {
        if venue == &self.cex {
            Ok(self.cex_network.clone())
        } else {
            super::transfers::network(core, asset, venue)
        }
    }
    pub fn network(&self, core: &Core, withdraw: bool) -> Result<CoinNetworkInfo, String> {
        self.token_network(core, &self.asset, withdraw)
    }
    pub fn token_network(
        &self,
        core: &Core,
        token: &Asset,
        withdraw: bool,
    ) -> Result<CoinNetworkInfo, String> {
        self.ensure(core)?;
        if token.contract.is_none() || token.chain != self.asset.chain {
            return Err("Токен находится вне сохранённой сети".into());
        }
        let network = &self.cex_network;
        let ticker = self.asset_on(core, token, &self.cex).ticker;
        let all = rd(&core.inner.networks);
        let r = unique(
            all.get(&self.cex)
                .map(|rows| rows.as_slice())
                .unwrap_or_default(),
            &self.cex,
            network,
            token,
        )?;
        if r.asset.ticker != ticker {
            return Err("Биржевой символ расчётного контракта изменился".into());
        }
        super::dirctx_venue::fresh(core, r)?;
        if (withdraw && !r.withdraw_enabled) || (!withdraw && !r.deposit_enabled) {
            return Err(format!(
                "{} {} в сети {} закрыт",
                if withdraw {
                    "Вывод"
                } else {
                    "Депозит"
                },
                ticker,
                network
            ));
        }
        Ok(r.clone())
    }
}

/// Физический адрес и выбранное биржей имя сети должны определять одну валюту этого счёта.
pub(super) fn unique<'a>(
    rows: &'a [CoinNetworkInfo],
    venue: &super::types::VenueId,
    network: &str,
    token: &Asset,
) -> Result<&'a CoinNetworkInfo, String> {
    let mut matching = rows.iter().filter(|r| {
        r.venue == *venue && r.network == network && r.asset == *token && token.contract.is_some()
    });
    let first = matching
        .next()
        .ok_or("Биржа не подтвердила сеть и контракт расчётного токена")?;
    if matching.next().is_some() || first.asset.ticker.trim().is_empty() {
        return Err("Контракт и сеть не определяют единственный биржевой тикер".into());
    }
    Ok(first)
}
