use alloy_primitives::{I256, U256};
use arb::core::convert::Rates;
use arb::core::costs_source::Source;
use arb::core::size::Fail;
use arb::core::types::{Cost, CostKind, Quote};
use arb::core::uncapped_replay_plan::dex_quote;
use arb::venues::bsc_dex::math::swap_step::step_v3;
use arb::venues::bsc_dex::num::{to_decimal, to_units};

struct FixedV3 {
    replay: Replay,
    buy_quote: Quote,
    sqrt: U256,
    boundary: U256,
    liquidity: u128,
    fee_pips: u32,
    capacity: U256,
    coin_decimals: u8,
    quote_decimals: u8,
    sell_depth: Decimal,
}

impl FixedV3 {
    fn exact_output(&self, qty: Decimal) -> Option<(U256, U256)> {
        let out = to_units(qty, self.coin_decimals)?;
        if out.is_zero() || out >= self.capacity || to_decimal(out, self.coin_decimals)? != qty {
            return None;
        }
        let signed = I256::try_from(out).ok()?.checked_neg()?;
        let (next, spent, received, fee) = step_v3(
            self.sqrt,
            self.boundary,
            self.liquidity,
            signed,
            self.fee_pips,
        )?;
        // TRADOOR buys token1 with token0. Keep the price strictly between
        // the saved Q96 state and nearest possible initialized lower tick.
        if !(self.boundary < next && next < self.sqrt && received == out) {
            return None;
        }
        Some((spent.checked_add(fee)?, fee))
    }
}

impl Source for FixedV3 {
    async fn quote(&self, dir: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        if sell {
            if qty > self.sell_depth {
                return Err(Fail::TooLarge(
                    "sale quantity exceeds saved BingX bids".into(),
                ));
            }
            return self.replay.quote(dir, sell, qty).await;
        }
        let (raw_input, raw_fee) = self.exact_output(qty).ok_or_else(|| {
            Fail::TooLarge("V3 quote is outside the proved single-tick interval".into())
        })?;
        let spent = to_decimal(raw_input, self.quote_decimals).ok_or_else(|| {
            Fail::TooLarge("V3 input cannot be represented as decimal USDT".into())
        })?;
        let fee = to_decimal(raw_fee, self.quote_decimals)
            .ok_or_else(|| Fail::TooLarge("V3 fee cannot be represented as decimal USDT".into()))?;
        let mut quote = dex_quote(&self.buy_quote, qty);
        quote.avg_price = spent / qty;
        if let Some(route) = &mut quote.route {
            route.legs[0].expected_out = qty;
        }
        for cost in &mut quote.costs {
            if cost.kind == CostKind::PoolFee {
                cost.amount = fee;
            }
        }
        Ok(quote)
    }

    fn worst(
        &self,
        dir: &Direction,
        sell: bool,
        qty: Decimal,
        avg: Decimal,
    ) -> Result<Decimal, Fail> {
        if sell {
            return self.replay.worst(dir, sell, qty, avg);
        }
        Ok(avg)
    }

    async fn withdraw(&self, cx: &DirCtx, qty: Decimal) -> Result<Vec<Cost>, Fail> {
        self.replay.withdraw(cx, qty).await
    }

    async fn ret(&self, cx: &DirCtx, qty: Decimal) -> Result<Vec<Cost>, Fail> {
        self.replay.ret(cx, qty).await
    }

    async fn rates(
        &self,
        cx: &DirCtx,
        coin: &str,
        price: Decimal,
        must: &[Cost],
    ) -> Result<Rates, Fail> {
        self.replay.rates(cx, coin, price, must).await
    }
}

fn u256(v: &Value) -> Result<U256, String> {
    U256::from_str_radix(v.as_str().ok_or("missing raw integer")?, 10).map_err(|e| e.to_string())
}

fn scale(v: &Value) -> Result<u8, String> {
    let n = v.as_u64().ok_or("missing verified decimals")?;
    u8::try_from(n).map_err(|e| e.to_string())
}

fn observed_step(plan: &Plan, book: &OrderBook) -> Decimal {
    let decimals = |q: Decimal| q.normalize().scale();
    let places = book
        .bids
        .iter()
        .map(|level| decimals(level.qty))
        .fold(decimals(plan.qty), u32::max)
        .min(18);
    Decimal::new(1, places)
}

async fn replay(case: &Value, budget: Decimal, max_evals: u32) -> Result<Value, String> {
    let mut direction = case["direction"].clone();
    let mut plan_data = case["plan"].clone();
    convert_asset(&mut direction["asset"]);
    convert_asset(&mut plan_data["buy_quote"]["asset"]);
    convert_asset(&mut plan_data["sell_quote"]["asset"]);
    let dir: Direction = serde_json::from_value(direction).map_err(|e| e.to_string())?;
    let plan = Plan::parse(&plan_data)?;
    let sell = read_book(&case["sell"], &dir, false)?;
    let empty_buy = OrderBook {
        venue: dir.buy_on.clone(),
        asset: dir.asset.clone(),
        bids: vec![],
        asks: vec![],
        exchange_time: None,
        received_at: sell.received_at,
    };
    let pool = &case["pool"];
    let source = FixedV3 {
        replay: Replay::new(
            &dir,
            &plan,
            empty_buy.clone(),
            Some(sell.clone()),
            ["USDT", "USDT"],
        )?,
        buy_quote: plan.buy_quote.clone(),
        sqrt: u256(&pool["sqrt_price_x96"])?,
        boundary: u256(&pool["boundary_sqrt_price_x96"])?,
        liquidity: pool["liquidity"]
            .as_str()
            .ok_or("missing V3 liquidity")?
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?,
        fee_pips: u32::try_from(pool["fee_pips"].as_u64().ok_or("missing V3 fee")?)
            .map_err(|e| e.to_string())?,
        capacity: u256(&pool["capacity_raw"])?,
        coin_decimals: scale(&pool["asset_decimals"])?,
        quote_decimals: scale(&pool["quote_decimals"])?,
        sell_depth: sell.bids.iter().map(|level| level.qty).sum(),
    };
    let expected = u256(&pool["quote_result_raw"])?;
    if source.exact_output(plan.qty).map(|(input, _)| input) != Some(expected) {
        return Err(
            "native V3 math does not reproduce archived input at the original exact output".into(),
        );
    }
    let max_qty = to_decimal(source.capacity, source.coin_decimals)
        .ok_or("single-tick ceiling does not fit decimal")?;
    let cx = context(&dir, &plan);
    let floor = (
        decimal(&case["thresholds"]["min_usdt"])?,
        decimal(&case["thresholds"]["min_pct"])?,
    );
    let mut curve = SourceCurve {
        cx: &cx,
        src: &source,
    };
    let own = curve.at(plan.qty).await;
    let mut p = params(&plan, &empty_buy, &sell, budget, floor, max_evals);
    let clipped_grid_limit = ((max_qty / p.step).ceil() - Decimal::ONE) * p.step;
    p.guess = p.guess.min(clipped_grid_limit).min(source.sell_depth);
    let found = search(&mut curve, &p).await;
    let best = found.as_ref().ok().map(|f| &f.best);
    if let Some(point) = best {
        assert!(
            point.qty < max_qty && point.invested <= budget && point.arrive <= source.sell_depth
        );
    }
    Ok(json!({
        "own_qty": plan.qty,
        "native_v3_raw_input_reproduced": expected.to_string(),
        "same_inputs_own_profit_usdt": own.as_ref().ok().map(|p| p.profit),
        "own_passes": own.as_ref().ok().is_some_and(|p| passes(&p.data, floor.0, floor.1)),
        "own_error": own.as_ref().err().map(|e| format!("{e:?}")),
        "search_error": found.as_ref().err().map(|e| format!("{e:?}")),
        "selected_qty": best.map(|p| p.qty),
        "selected_spend_max_usdt": best.map(|p| p.invested),
        "selected_profit_usdt": best.map(|p| p.profit),
        "passes": best.is_some_and(|p| passes(&p.data, floor.0, floor.1)),
        "evaluated": found.as_ref().ok().map(|f| f.choice.evals),
        "single_tick_boundary_qty_exclusive": max_qty,
        "single_tick_grid_limit_qty": clipped_grid_limit,
        "observed_grid_step": p.step,
        "recorded_bid_depth_qty": source.sell_depth,
        "recorded_bid_levels": sell.bids.len(),
        "threshold_placement": "NATIVE_THRESHOLD_PLACEMENT",
    }))
}
