//! Файл: gateway/aliased_venue.rs
//! Что делает: решение 200 — контракт Venue обёртки склейки тикеров (gateway/aliased.rs): каждый вызов к бирже — с
//! тикером биржи (aliased_tr out_*), каждый ответ и событие — с основным тикером (back_*); остальное — как есть.
//! coin_networks заодно пересчитывает карту переименований биржи (Aliased::refresh). Методы DEX биржа не реализует —
//! у обёртки ответы по умолчанию контракта (NotApplicable), как у самой биржи.
//! Кто вызывает: ядро и API через gateway/registry.rs.
//! От чего зависит: gateway/aliased.rs, aliased_tr.rs, contract.rs, events.rs, core/types.
//! Настройки: нет своих.

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    AccountBalances, Asset, Balance, CoinNetworkInfo, Cost, Quote, SendChannelInfo, Side,
    TradeRequest, TradeResult, Transfer, VenueId, WithdrawRequest,
};

use super::aliased::Aliased;
use super::aliased_tr as tr;
use super::contract::{AccountRole, ConnectionCheck, DexFamily, Venue, VenueKind};
use super::errors::GatewayResult;
use super::events::MarketEvent;

#[async_trait]
impl Venue for Aliased {
    fn id(&self) -> VenueId {
        self.inner.id()
    }
    fn kind(&self) -> VenueKind {
        self.inner.kind()
    }
    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        self.inner.check_connection().await
    }
    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        let raw = match self.inner.coin_networks().await {
            Ok(r) => r,
            Err(e) => return Err(self.with_hint(e).await),
        };
        *self.bsc_hint.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.refresh(&raw).await;
        Ok(tr::back_networks(&self.m(), raw))
    }
    async fn coin_networks_of(&self, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
        let m = self.m();
        let raw = self
            .inner
            .coin_networks_of(&tr::out_str(&m, ticker))
            .await?;
        Ok(tr::back_networks(&m, raw))
    }
    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        let m = self.m();
        if m.is_empty() {
            return self.inner.subscribe_market(assets, out).await;
        }
        let (tx, mut rx) = mpsc::channel(out.max_capacity());
        let fwd = m.clone();
        tokio::spawn(async move {
            while let Some(e) = rx.recv().await {
                if out.send(tr::back_event(&fwd, e)).await.is_err() {
                    break;
                }
            }
        });
        let list = assets.iter().map(|a| tr::out_asset(&m, a)).collect();
        self.inner.subscribe_market(list, tx).await
    }
    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        let m = self.m();
        let q = self
            .inner
            .quote(&tr::out_asset(&m, asset), side, qty)
            .await?;
        Ok(tr::back_quote(&m, q))
    }
    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        let m = self.m();
        Ok(tr::back_trade(
            &m,
            self.inner.trade(tr::out_trade(&m, req)).await?,
        ))
    }
    async fn check_order(&self, req: &TradeRequest) -> GatewayResult<()> {
        self.inner
            .check_order(&tr::out_trade(&self.m(), req.clone()))
            .await
    }
    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        let m = self.m();
        Ok(tr::back_transfer(
            &m,
            self.inner.withdraw(tr::out_withdraw(&m, req)).await?,
        ))
    }
    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        Ok(tr::back_transfer(
            &self.m(),
            self.inner.withdrawal_status(venue_ref).await?,
        ))
    }
    async fn deposit_address(&self, asset: &Asset, network: &str) -> GatewayResult<String> {
        self.inner
            .deposit_address(&tr::out_asset(&self.m(), asset), network)
            .await
    }
    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        let m = self.m();
        let t = self
            .inner
            .deposit_status(&tr::out_asset(&m, asset), tx_hash)
            .await?;
        Ok(tr::back_transfer(&m, t))
    }
    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        let m = self.m();
        Ok(self
            .inner
            .balances()
            .await?
            .into_iter()
            .map(|b| tr::back_balance(&m, b))
            .collect())
    }
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        Ok(tr::back_accounts(
            &self.m(),
            self.inner.account_balances().await?,
        ))
    }
    fn account_role(&self, account: &str) -> AccountRole {
        self.inner.account_role(account)
    }
    async fn balance(&self, asset: &str) -> GatewayResult<Option<Balance>> {
        let m = self.m();
        let b = self.inner.balance(&tr::out_str(&m, asset)).await?;
        Ok(b.map(|b| tr::back_balance(&m, b)))
    }
    async fn withdraw_hold(&self, quote: &str) -> GatewayResult<crate::core::types::WithdrawHold> {
        self.inner.withdraw_hold(quote).await
    }
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        self.inner.delistings().await
    }
    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        let m = self.m();
        let c = self
            .inner
            .withdraw_cost(&tr::out_asset(&m, asset), network, amount)
            .await?;
        Ok(tr::back_costs(&m, c))
    }
    async fn recheck_coins(&self, assets: Vec<Asset>) -> GatewayResult<()> {
        let m = self.m();
        self.inner
            .recheck_coins(assets.iter().map(|a| tr::out_asset(&m, a)).collect())
            .await
    }
    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        self.inner.dex_families().await
    }
    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        self.inner.send_channels().await
    }
    async fn check_send_channel(&self, id: &str) -> GatewayResult<ConnectionCheck> {
        self.inner.check_send_channel(id).await
    }
    async fn deposit_abandoned(&self, asset: &Asset) -> GatewayResult<()> {
        self.inner
            .deposit_abandoned(&tr::out_asset(&self.m(), asset))
            .await
    }
    async fn move_to_trading(&self, asset: &Asset, amount: Decimal) -> GatewayResult<String> {
        self.inner
            .move_to_trading(&tr::out_asset(&self.m(), asset), amount)
            .await
    }
    async fn deposit_to_trading(
        &self,
        asset: &Asset,
        amount: Decimal,
    ) -> GatewayResult<Option<String>> {
        self.inner
            .deposit_to_trading(&tr::out_asset(&self.m(), asset), amount)
            .await
    }
    async fn window_tap(
        &self,
        asset: &Asset,
        sink: super::tap::TapSink,
    ) -> GatewayResult<super::tap::TapInfo> {
        self.inner
            .window_tap(&tr::out_asset(&self.m(), asset), sink)
            .await
    }
    async fn window_untap(&self, asset: &Asset) -> GatewayResult<()> {
        self.inner
            .window_untap(&tr::out_asset(&self.m(), asset))
            .await
    }
    fn decode_tap(
        &self,
        stream: super::tap::TapStream,
        data: &super::tap::TapData,
    ) -> GatewayResult<crate::core::types::TapDecoded> {
        self.inner.decode_tap(stream, data)
    }
    // Решение 202 через обёртку склейки тикеров (решение 200): тикеры пар — обратно в имена ядра, команды — в имя
    // биржи; без этих методов обёртка подставила бы реализацию по умолчанию «не умею» и выключила сторож.
    fn feed_health(&self) -> Option<super::feed_health::FeedHealth> {
        let mut h = self.inner.feed_health()?;
        let m = self.m();
        for c in &mut h.conns {
            for p in &mut c.pairs {
                p.ticker = tr::back_str(&m, &p.ticker);
            }
        }
        Some(h)
    }
    async fn resubscribe_pair(&self, conn: &str, ticker: &str) -> GatewayResult<()> {
        self.inner
            .resubscribe_pair(conn, &tr::out_str(&self.m(), ticker))
            .await
    }
    async fn reconnect_conn(&self, conn: &str) -> GatewayResult<()> {
        self.inner.reconnect_conn(conn).await
    }
}
