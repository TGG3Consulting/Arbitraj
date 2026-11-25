"""The closed-card comparator has no venue rule flags. Restore its observed CEX buy side
for the archived fast check. Historical order-size grid is the comparator's saved data
grid, not a claim that a complete historical symbol filter was captured."""
from pathlib import Path
import sys
p=Path(sys.argv[1])/'tests/core_size_actual_v2.rs';s=p.read_text()
a='let cx = context(&dir, &plan, decimal(&cfg["profit.price_move_k"])?, decimal(&cfg["profit.liquidity_share_h"])?);'
assert s.count(a)==1
s=s.replace(a,a.replace('let cx','let mut cx')+'''
    assert!(matches!((dir.asset.ticker.as_str(),dir.buy_on.0.as_str(),dir.sell_on.0.as_str()),("DEOD","poloniex","bsc_dex")|("RT","mexc","bsc_dex")));
    cx.pair_a=Some(serde_json::from_value(json!({"ticker":dir.asset.ticker,"online":true,"status":1,"api_buy":true,"api_sell":true,"step_size":step_of(&plan,&a)})).unwrap());
''')
a='    let mut old_curve = SourceCurve { cx: &old_ctx, src: &source };';assert s.count(a)==1
p.write_text(s.replace(a,'    old_ctx.pair_a=cx.pair_a.clone();\n'+a))
