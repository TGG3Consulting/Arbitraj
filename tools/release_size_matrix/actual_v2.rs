use alloy_primitives::U256;
use arb::core::convert::Rates;
use arb::core::costs_source::Source;
use arb::core::size::Fail;
use arb::core::types::{Cost, CostKind, Quote};
use arb::venues::bsc_dex::num::{to_decimal, to_units};

struct FixedV2 {
    replay: Replay,
    max_buy_qty: Decimal,
    reserve_in: U256,
    reserve_out: U256,
    decimals_in: u8,
    decimals_out: u8,
}

impl FixedV2 {
    fn amount_out(&self, qty: Decimal) -> Option<U256> {
        let received = to_units(qty, self.decimals_in)?;
        let effective = received.checked_mul(U256::from(9975u32))?;
        let numerator = effective.checked_mul(self.reserve_out)?;
        let denominator = self.reserve_in.checked_mul(U256::from(10_000u32))?.checked_add(effective)?;
        (!denominator.is_zero()).then(|| numerator / denominator)
    }
}

impl Source for FixedV2 {
    async fn quote(&self, dir: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        if !sell {
            if qty > self.max_buy_qty {
                return Err(Fail::TooLarge("buy quantity exceeds the recorded CEX asks".into()));
            }
            return self.replay.quote(dir, sell, qty).await;
        }
        let raw = self.amount_out(qty).ok_or_else(|| Fail::TooLarge("V2 quantity overflow".into()))?;
        let out = to_decimal(raw, self.decimals_out)
            .filter(|out| *out > Decimal::ZERO)
            .ok_or_else(|| Fail::TooSmall("zero V2 output".into()))?;
        // Native direct V2 reports the requested quantity for a complete swap.
        // Only the contract input is rounded to atomic units.
        let mut quote = self.replay.quote(dir, sell, qty).await?;
        quote.fillable_qty = qty;
        quote.avg_price = out / qty;
        for cost in &mut quote.costs {
            if cost.kind == CostKind::PoolFee {
                cost.amount = qty * Decimal::new(25, 4);
            }
        }
        if let Some(route) = &mut quote.route {
            route.legs[0].expected_out = out;
        }
        Ok(quote)
    }

    fn worst(&self, dir: &Direction, sell: bool, qty: Decimal, avg: Decimal) -> Result<Decimal, Fail> {
        self.replay.worst(dir, sell, qty, avg)
    }

    async fn withdraw(&self, cx: &DirCtx, qty: Decimal) -> Result<Vec<Cost>, Fail> {
        self.replay.withdraw(cx, qty).await
    }

    async fn ret(&self, cx: &DirCtx, qty: Decimal) -> Result<Vec<Cost>, Fail> {
        self.replay.ret(cx, qty).await
    }

    async fn rates(&self, cx: &DirCtx, coin: &str, price: Decimal, must: &[Cost]) -> Result<Rates, Fail> {
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

async fn replay(case: &Value, budget: Decimal, max_evals: u32) -> Result<Value, String> {
    let mut direction = case["direction"].clone();
    let mut plan = case["plan"].clone();
    convert_asset(&mut direction["asset"]);
    convert_asset(&mut plan["buy_quote"]["asset"]);
    convert_asset(&mut plan["sell_quote"]["asset"]);
    let dir: Direction = serde_json::from_value(direction).map_err(|e| e.to_string())?;
    let plan = Plan::parse(&plan)?;
    let a = read_book(&case["buy"], &dir, true)?;
    let pool = &case["pool"];
    let source = FixedV2 {
        replay: Replay::new(&dir, &plan, a.clone(), None, ["USDT", "USDT"])?,
        max_buy_qty: a.asks.iter().map(|level| level.qty).sum(),
        reserve_in: u256(&pool["coin_reserve_raw"])?,
        reserve_out: u256(&pool["quote_reserve_raw"])?,
        decimals_in: scale(&pool["coin_decimals"])?,
        decimals_out: scale(&pool["quote_decimals"])?,
    };
    if source.amount_out(plan.sell_qty) != Some(u256(&pool["planned_out_raw"])?) {
        return Err("fixed-route reserve replay does not reproduce original input/output".into());
    }
    let cx = context(&dir, &plan);
    let floor = (d("0.05"), Decimal::ZERO);
    let mut curve = SourceCurve { cx: &cx, src: &source };
    let own = curve.at(plan.qty).await;
    // The saved episode contains only the configured prefix of the CEX book.
    // The DEX fixed path has no CEX sale breakpoints.
    let empty_sell = OrderBook {
        venue: dir.sell_on.clone(),
        asset: dir.asset.clone(),
        bids: vec![],
        asks: vec![],
        exchange_time: None,
        received_at: a.received_at,
    };
    let mut p = params(&plan, &a, &empty_sell, budget, floor, max_evals);
    p.guess = p.guess.min(source.max_buy_qty);
    let found = search(&mut curve, &p).await;
    let best = found.as_ref().ok().map(|f| &f.best);
    if let Some(point) = best {
        assert!(point.qty <= source.max_buy_qty && point.invested <= budget);
    }
    Ok(json!({
        "own_qty": plan.qty,
        "same_inputs_own_profit_usdt": own.as_ref().ok().map(|p| p.profit),
        "own_error": own.as_ref().err().map(|e| format!("{e:?}")),
        "search_error": found.as_ref().err().map(|e| format!("{e:?}")),
        "selected_qty": best.map(|p| p.qty),
        "selected_spend_max_usdt": best.map(|p| p.invested),
        "selected_profit_usdt": best.map(|p| p.profit),
        "passes": best.is_some_and(|p| passes(&p.data, floor.0, floor.1)),
        "recorded_ask_depth_qty": source.max_buy_qty,
        "recorded_ask_levels": a.asks.len(),
        "threshold_placement": "NATIVE_THRESHOLD_PLACEMENT",
    }))
}
