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
        let ticker = super::venue_ticker::ticker_on(core, token, &self.cex);
        let all = rd(&core.inner.networks);
        let mut rows = all
            .get(&self.cex)
            .into_iter()
            .flat_map(|r| r.iter())
            .filter(|r| r.venue == self.cex && &r.network == network && r.asset == *token);
        let r = rows
            .next()
            .ok_or("Биржа не подтвердила сеть и контракт расчётного токена")?;
        if rows.next().is_some() || r.asset.ticker != ticker {
            return Err("Неоднозначная запись расчётного токена".into());
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
