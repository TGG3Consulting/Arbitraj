//! Архив содержит две котировки на количества плана. В replay используем эти
//! цены на плоских книгах. Глубина и неизвестные лимиты — условия эксперимента.
use arb::core::dirctx::DirCtx;
use arb::core::frozen::Frozen;
use arb::core::types::{Asset, CoinNetworkInfo, Direction, OrderBook, PriceLevel, Side, VenueId};
use arb::gateway::events::SpotPairState;
use arb::gateway::frozen::{BookFee, FrozenBook};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;
use std::sync::Arc;

use crate::{d, sieve_common};

pub fn num(object: &Value, key: &str) -> Decimal {
    d(object[key].as_str().unwrap())
}

fn pair(ticker: &str, step: Decimal) -> SpotPairState {
    SpotPairState {
        ticker: ticker.into(),
        status: 1,
        online: true,
        api_buy: true,
        api_sell: true,
        step_size: step,
        min_notional: None,
        max_notional: None,
        min_qty: None,
        max_qty: None,
        order_block: None,
        price_band: None,
    }
}

fn network(asset: &Asset, venue: &VenueId, at: DateTime<Utc>) -> CoinNetworkInfo {
    CoinNetworkInfo {
        venue: venue.clone(),
        asset: asset.clone(),
        network: "BEP20".into(),
        deposit_enabled: true,
        withdraw_enabled: true,
        withdraw_fee: None,
        use_net_withdrawal_budget: false,
        withdraw_fee_pct: None,
        withdraw_min: None,
        withdraw_max: None,
        deposit_min: Some(Decimal::ZERO),
        deposit_confirmations: None,
        withdraw_precision: None,
        withdraw_step: None,
        updated_at: at,
    }
}

pub struct Recording {
    pub cx: DirCtx,
    pub books: Frozen,
    pub raw: Value,
    pub step: Decimal,
    pub fee: Decimal,
}

impl Recording {
    pub fn load() -> Self {
        let raw: Value =
            serde_json::from_str(include_str!("../data/mub_plateau_2026-11-09.json")).unwrap();
        let (plan, model, costs) = (&raw["plan"], &raw["reconstruction"], &raw["costs"]);
        let at: DateTime<Utc> = plan["at"].as_str().unwrap().parse().unwrap();
        let dir: Direction = serde_json::from_value(plan["direction"].clone()).unwrap();
        let step = num(model, "buy_step");
        let mut from = network(&dir.asset, &dir.buy_on, at);
        from.withdraw_precision = Some(
            model["withdraw_precision"]
                .as_u64()
                .unwrap()
                .try_into()
                .unwrap(),
        );
        from.withdraw_fee = Some(num(costs, "WithdrawFee_MUB"));
        let mut a = sieve_common::presence(&from, true);
        a.venue = dir.buy_on.clone();
        a.ticker = dir.asset.ticker.clone();
        let mut b = sieve_common::presence(&network(&dir.asset, &dir.sell_on, at), true);
        b.venue = dir.sell_on.clone();
        b.ticker = dir.asset.ticker.clone();
        let usdt = Asset {
            ticker: "USDT".into(),
            chain: None,
            contract: None,
        };
        let mut ret = network(&usdt, &dir.sell_on, at);
        ret.withdraw_fee = Some(num(costs, "UsdtReturn_USDT"));
        let mut pair_a = pair(&dir.asset.ticker, step);
        pair_a.min_qty = Some(num(&raw["size"], "min_qty"));
        let pair_b = pair(&dir.asset.ticker, num(model, "sell_step"));
        let fee = num(costs, "TradeFee_MUB") / num(plan, "qty");
        let proceeds = num(plan, "proceeds");
        let leg = |side: Side, index: usize, rate: Decimal, venue: &VenueId, price: Decimal| {
            let taken_at = raw["quote_times"][index].as_str().unwrap().parse().unwrap();
            let levels = vec![PriceLevel {
                price,
                qty: num(model, "single_level_depth"),
            }];
            Arc::new(FrozenBook {
                venue: venue.clone(),
                asset: dir.asset.clone(),
                side,
                taken_at,
                fee: BookFee::in_received(rate, side, &dir.asset.ticker, "USDT"),
                book: OrderBook {
                    venue: venue.clone(),
                    asset: dir.asset.clone(),
                    exchange_time: None,
                    received_at: taken_at,
                    bids: levels.clone(),
                    asks: levels,
                },
            })
        };
        let books = Frozen {
            a: leg(Side::Buy, 0, fee, &dir.buy_on, num(plan, "buy_avg")),
            b: leg(
                Side::Sell,
                1,
                num(costs, "TradeFee_USDT") / proceeds,
                &dir.sell_on,
                num(plan, "sell_avg"),
            ),
            at,
            decided: None,
        };
        let cx = DirCtx {
            units_b_per_a: Default::default(),
            dir,
            a,
            b,
            net_a: "BEP20".into(),
            net_b: "BEP20".into(),
            quote: vec!["USDT".into(), "USDT".into()],
            usdt,
            cex: vec![],
            sigma: num(plan, "sigma"),
            transit_s: num(plan, "transit_s"),
            // k не сообщён. Сам costs::evaluate* использует keep, восстановленный
            // из строки резерва в плане. Значение k в этой проверке не читается.
            k: Decimal::ZERO,
            keep: Some(Decimal::ONE - num(costs, "PriceMoveReserve_USDT") / proceeds),
            sigma_check: None,
            h: num(model, "h"),
            closure_share: None,
            return_per_trade: true,
            pair_a: Some(pair_a),
            pair_b: Some(pair_b),
            usdt_b: Some(ret),
            transfer_tax: Some(Decimal::ZERO),
            sell_max: None,
            band_a: None,
            band_b: None,
        };
        Self {
            cx,
            books,
            raw,
            step,
            fee,
        }
    }
}

/// Зафиксированный пример изменённых тарифов и двух удержаний. Направление
/// и котировки делегируются штатному снимку. Факты перевода заданы явно.
pub struct TransferReplay<'a> {
    pub live: arb::core::costs::Live<'a>,
    pub seen: arb::core::withdraw_seen::Seen,
    pub hold_pct: Decimal,
}

use arb::core::convert::Rates;
use arb::core::costs::Source;
use arb::core::size::Fail;
use arb::core::types::{Cost, Quote};

impl Source for TransferReplay<'_> {
    async fn quote(&self, dir: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        self.live.quote(dir, sell, qty).await
    }
    fn worst(
        &self,
        dir: &Direction,
        sell: bool,
        qty: Decimal,
        avg: Decimal,
    ) -> Result<Decimal, Fail> {
        self.live.worst(dir, sell, qty, avg)
    }
    async fn withdraw(&self, cx: &DirCtx, sent: Decimal) -> Result<Vec<Cost>, Fail> {
        let initial = self.live.withdraw(cx, sent).await?;
        let info = cx.a.info.as_ref().unwrap();
        let mut adjusted = arb::core::withdraw_seen::model(
            &self.seen,
            info.fee_for(self.seen.sent),
            cx.transfer_tax.unwrap(),
            sent,
            initial,
        );
        let taken: Decimal = adjusted
            .iter()
            .filter(|c| !c.in_price && arb::core::convert::in_coin(c, &cx.dir.asset.ticker))
            .map(|c| c.amount)
            .sum();
        adjusted.extend(arb::core::deposit_hold::line(
            self.hold_pct,
            sent - taken,
            &cx.dir.asset.ticker,
        ));
        Ok(adjusted)
    }
    async fn ret(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        self.live.ret(cx, amount).await
    }
    async fn rates(
        &self,
        cx: &DirCtx,
        coin: &str,
        price: Decimal,
        must: &[Cost],
    ) -> Result<Rates, Fail> {
        self.live.rates(cx, coin, price, must).await
    }
}
