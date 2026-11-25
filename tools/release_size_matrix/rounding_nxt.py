from pathlib import Path
import sys, json
t=Path(sys.argv[1])/'tests'; tools=Path(__file__).parent
p=t/'core_size_stored_compare.rs'; source=p.read_text()
loc='let floor = (decimal(&cfg'
assert loc in source
src=r"""
    // Sensitivity: historical quantities pin these precision choices at the original amount;
    // minimums and metadata at other quantities are unknown.
    let (precision, sell_step): (Option<u32>, &str) = match dir.asset.ticker.as_str() {
      "EVAA" => (Some(1),"0.01"), "RAIN" => (None,"0.1"), "AI" => (Some(2),"0.1"),
      _ => panic!("no evidence for rounding model")
    };
    cx.a.info = Some(serde_json::from_value(json!({"venue":dir.buy_on,"asset":dir.asset,"network":"archived-control",
      "deposit_enabled":true,"withdraw_enabled":true,"withdraw_precision":precision,"updated_at":a.received_at})).unwrap());
    cx.pair_b = Some(serde_json::from_value(json!({"ticker":dir.asset.ticker,"status":1,"online":true,
      "api_buy":true,"api_sell":true,"step_size":sell_step})).unwrap());
    """
source=source.replace('let cx = context(', 'let mut cx = context(').replace(loc,src+loc)
source=source.replace('"neighbors": neighbors','"rounding": {"withdraw_decimals":precision,"sell_step":sell_step,"basis":"inferred from historic fixed quantity; no historical venue rule snapshot"}, "neighbors": neighbors')
p.write_text(source)
# Stage a fixed-route V3 sell over the checked lower/upper Q96 interval. The CEX buy,
# cost calculations, historical settings and size selection remain unchanged.
p=t/'core_size_actual_v2.rs'
nxt=p.read_text().replace('use alloy_primitives::U256;','use alloy_primitives::{U256,I256};\nuse arb::venues::bsc_dex::math::swap_step::step_v3;')
a=nxt.index('    reserve_in: U256,'); b=nxt.index('\nimpl Source for FixedV2',a)
nxt=nxt[:a]+r"""    sqrt: U256, boundary: U256, liquidity: u128, fee_pips: u32, decimals_in: u8, decimals_out: u8,
}

impl FixedV2 {
    fn calculate(&self, qty: Decimal) -> Option<(U256,U256)> {
        let input=to_units(qty,self.decimals_in)?;
        // The live Sell path rounds contract input down to atomic units and reports the
        // original Decimal quantity when the swap completes (direct.rs, route_calc.rs).
        if input.is_zero() {return None}
        let (next,spent,output,fee)=step_v3(self.sqrt,self.boundary,self.liquidity,I256::try_from(input).ok()?,self.fee_pips)?;
        if !(self.sqrt<next && next<self.boundary && spent.checked_add(fee)?==input) {return None}
        Some((output,fee))
    }
}
"""+nxt[b:]
nxt=nxt.replace('let raw = self.amount_out(qty)', 'let (raw, fee) = self.calculate(qty)').replace('V2 quantity overflow','outside strictly proved V3 price interval or precision').replace('cost.amount = qty * Decimal::new(25, 4);','cost.amount = to_decimal(fee,self.decimals_in).ok_or_else(||Fail::TooLarge("fee conversion".into()))?;')
a=nxt.index('        reserve_in:'); b=nxt.index('    };',a)
nxt=nxt[:a]+r"""        sqrt:u256(&pool["sqrt_price_x96"])?, boundary:u256(&pool["boundary_sqrt_price_x96"])?,
        liquidity:pool["liquidity"].as_str().unwrap().parse::<u128>().unwrap(),
        fee_pips:u32::try_from(pool["fee_pips"].as_u64().unwrap()).unwrap(),
        decimals_in: scale(&pool["asset_decimals"])?, decimals_out:scale(&pool["quote_decimals"])?,
"""+nxt[b:]
nxt=nxt.replace('if source.amount_out(plan.sell_qty) != Some(u256(&pool["planned_out_raw"])?) {','assert_eq!(pool["zero_for_one"],false);\n    if source.calculate(plan.sell_qty).map(|v|v.0) != Some(u256(&pool["quote_result_raw"])?) {')
nxt=nxt.replace('size_actual_v2_cases.json','control_nxt_cases.json').replace('ACTUAL_V2_MATRIX','NXT_MATRIX').replace('actual_v2_matrix','nxt_control')
nxt=nxt.replace('let own = curve.at(plan.qty).await;', 'let own = curve.at(plan.qty).await; assert!(own.is_ok(), "saved NXT size must quote at both nominal and depth-check quantity: {:?}", own.as_ref().err());')
p2=t/'control_nxt.rs';p2.write_text(nxt)
c=json.loads((tools/'nxt_control.json').read_text(encoding='utf-8-sig'))['cases']
for r in c: r['control']=json.loads((tools/'control_nxt_settings.json').read_text(encoding='utf-8-sig'))
(t/'control_nxt_cases.json').write_text(json.dumps(c))
