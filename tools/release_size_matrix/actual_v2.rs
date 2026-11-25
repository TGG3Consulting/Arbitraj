use alloy_primitives::{I256, U256};
use arb::venues::bsc_dex::math::swap_step::step_v3;
use arb::core::convert::Rates;
use arb::core::costs_source::Source;
use arb::core::size::Fail;
use arb::core::types::{Cost, CostKind, Quote};
use arb::venues::bsc_dex::num::{to_decimal, to_units};

enum FrozenPool {
    V2 { reserve_in: U256, reserve_out: U256 },
    V3 { sqrt: U256, boundary: U256, liquidity: u128, fee: u32 },
}
struct FixedV2 {
    replay: Replay,
    max_buy_qty: Decimal,
    frozen: FrozenPool,
    decimals_in: u8,
    decimals_out: u8,
}

impl FixedV2 {
    fn swap(&self, qty: Decimal) -> Option<(U256, Decimal)> {
        let received = to_units(qty, self.decimals_in)?;
        if received.is_zero() { return None; }
        match self.frozen {
            FrozenPool::V2 { reserve_in, reserve_out } => {
                let effective = received.checked_mul(U256::from(9975u32))?;
                let numerator = effective.checked_mul(reserve_out)?;
                let denominator = reserve_in.checked_mul(U256::from(10_000u32))?.checked_add(effective)?;
                Some((numerator.checked_div(denominator)?, qty.checked_mul(Decimal::new(25, 4))?))
            },
            FrozenPool::V3 { sqrt, boundary, liquidity, fee } => {
                let signed = I256::try_from(received).ok()?;
                let (next, used, out, paid) = step_v3(sqrt, boundary, liquidity, signed, fee)?;
                // No tick bitmap or liquidityNet outside this first open interval was recorded.
                if next <= sqrt.min(boundary) || next >= sqrt.max(boundary) ||
                    used.checked_add(paid)? != received { return None; }
                Some((out, to_decimal(paid, self.decimals_in)?))
            }
        }
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
        let (raw, pool_fee) = self.swap(qty).ok_or_else(|| Fail::TooLarge("V2 quantity overflow".into()))?;
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
                cost.amount = pool_fee;
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
        frozen: if pool["kind"] == "v3" {
            FrozenPool::V3 {
                sqrt: u256(&pool["sqrt"])?,
                boundary: u256(&pool["boundary"])?,
                liquidity: pool["liquidity"].as_str().ok_or("missing liquidity")?.parse::<u128>().map_err(|e| e.to_string())?,
                fee: u32::try_from(pool["fee"].as_u64().ok_or("missing fee")?).map_err(|e|e.to_string())?,
            }
        } else {
            FrozenPool::V2 { reserve_in: u256(&pool["coin_reserve_raw"])?, reserve_out: u256(&pool["quote_reserve_raw"])? }
        },
        decimals_in: scale(&pool["coin_decimals"])?,
        decimals_out: scale(&pool["quote_decimals"])?,
    };
    if source.swap(plan.sell_qty).map(|x|x.0) != Some(u256(&pool["planned_out_raw"])?) {
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
