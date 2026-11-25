from pathlib import Path
import json,re,sys
root=Path(sys.argv[1])/"tests"
tools=Path(__file__).parent
settings=json.loads((tools/"control_settings.json").read_text(encoding="utf-8-sig"))
for lane in ["stored_compare","actual_v2","actual_v3"]:
    name="stored" if lane=="stored_compare" else lane
    p=root/f"size_{name}_cases.json"
    cases=json.loads(p.read_text())
    for case in cases:
        case["control"]=settings[case["deal_id"]]
    p.write_text(json.dumps(cases))
    p=root/f"core_size_{lane}.rs"
    src=p.read_text()
    def swap(old,new):
        nonlocal_placeholder=None
        global src
        assert src.count(old)==1,(lane,old)
        src=src.replace(old,new)
    swap("fn context(dir: &Direction, plan: &Plan)", "fn context(dir: &Direction, plan: &Plan, k: Decimal, h: Decimal)")
    swap('let (k, h, closure_share) = (d("3.6"), d("0.7"), Some(d("0.8")));', 'let closure_share: Option<Decimal> = None;')
    swap('let cx = context(&dir, &plan);','let cfg = &case["control"]; assert!(cfg["profit.closure_share"].is_null()); let cx = context(&dir, &plan, decimal(&cfg["profit.price_move_k"])?, decimal(&cfg["profit.liquidity_share_h"])?);')
    src=re.sub(r'let floor = (?:\(d\("0.05"\), Decimal::ZERO\);|\(\s*decimal\(&case\["thresholds"\]\["min_usdt"\]\)\?,\s*decimal\(&case\["thresholds"\]\["min_pct"\]\)\?,\s*\);)', 'let floor = (decimal(&cfg["money.min_profit_usd"])?, decimal(&cfg["money.min_profit_pct"])?);',src)
    assert 'let floor = (decimal(&cfg' in src,lane
    start=src.index('let '+('reference' if lane=='stored_compare' else 'own')+' = curve.at(plan.qty).await;')
    neighbors='''
    let mut neighbors=Vec::new();
    for factor in ["0.8","0.99","1","1.01","1.2"] {
        let qty=(plan.qty*d(factor)).round_dp_with_strategy(plan.qty.normalize().scale(),rust_decimal::RoundingStrategy::ToZero);
        let result=match curve.at(qty).await {
            Ok(pt)=>json!({"model_profit_usdt":pt.profit,"spend_usdt":pt.invested,"within_budget":pt.invested<=budget,"passes":pt.invested<=budget && passes(&pt.data,floor.0,floor.1)}),
            Err(e)=>json!({"error":format!("{e:?}")}),
        };
        neighbors.push(json!({"factor":factor,"qty":qty,"result":result}));
    }
    '''
    src=src[:start]+neighbors+src[start:]
    swap('"own_qty": plan.qty','"neighbors": neighbors, "own_qty": plan.qty')
    src=re.sub(r'"model": \{"k": "3.6", .*?"eval_limit": max_evals\}', '"model": {"asof": case["control"], "eval_limit": max_evals}',src)
    assert '"model": {"asof"' in src,lane
    p.write_text(src)
