    use arb::core::{reserve, size_memo::Memo, size_search::search_in};
    use std::time::Instant;

    // Identical fixed pool, book, fees, direction and time in transit throughout.
    // The full historical graph and its historical 127/141 tested quantities were not saved.
    let earlier = &case["open_evidence"];
    let mut old_ctx = context(&dir, &plan, decimal(&cfg["profit.price_move_k"])?, decimal(&cfg["profit.liquidity_share_h"])?);
    old_ctx.sigma = decimal(&earlier["sigma"])?;
    old_ctx.keep = reserve::keep_of(decimal(&cfg["profit.price_move_k"])?,old_ctx.sigma,old_ctx.transit_s);
    let mut old_curve = SourceCurve { cx: &old_ctx, src: &source };
    let mut old_p = params(&plan, &a, &empty_sell, decimal(&earlier["budget"])?, floor, max_evals);
    old_p.guess = old_p.guess.min(source.max_buy_qty);
    let mut memo=Memo::new(&mut old_curve,old_p.step,old_p.budget,max_evals)
        .with_share(old_p.par_points,old_p.reuse)
        .with_gate(old_p.ext.min_usd,old_p.ext.min_pct);
    let reconstructed = search_in(&mut memo,&old_p).await.map_err(|e| e.to_string())?;
    let mut points:Vec<_> = memo.map.values().flatten().map(|point| point.data.clone()).collect();
    let old_points=points.len();
    // Include the real incumbent even if the bounded replay generated another grid.
    if !points.iter().any(|c|c.qty==plan.qty) {
        let extra=old_curve.at(plan.qty).await.map_err(|e|e.to_string())?;
        points.push(extra.data);
    }
    let keep=cx.keep.unwrap();
    let compact:Vec<_> = points.iter().map(|c|{
        let r=c.costs.iter().find(|l|l.kind==CostKind::PriceMoveReserve).unwrap().usdt.unwrap();
        (c.qty,c.spend_max,c.invested,c.proceeds,c.profit_usd+r,c.buy_avg,c.sell_avg)
    }).collect();
    let rank = ||{
        let mut best:Option<(Decimal,Decimal,Decimal)>=None;
        for &(qty,spend,base,proceeds,pre,buy,sell) in &compact {
            if spend>budget || base<=Decimal::ZERO {continue;}
            let profit=pre-reserve::amount(keep,proceeds,cx.closure_share,buy,sell);
            if profit<floor.0 || profit/base*d("100")<floor.1 {continue;}
            if best.is_none_or(|(q,s,p)|profit>p || (profit==p && (spend,q)<(s,q))) {best=Some((qty,spend,profit));}
        }
        best.unwrap()
    };
    // Compare each re-rated point with an independent native fresh calculation.
    let mut max_diff=Decimal::ZERO;
    for &(qty,_,_,proceeds,pre,buy,sell) in &compact {
        let f=curve.at(qty).await.map_err(|e|e.to_string())?;
        let re=pre-reserve::amount(keep,proceeds,cx.closure_share,buy,sell);
        max_diff=max_diff.max((re-f.profit).abs());
    }
    assert!(max_diff<d("0.00000000001"),"invariance of risk-only re-rating");
    let (pick,_,rerated)=rank();
    let fresh=curve.at(pick).await.map_err(|e|e.to_string())?;
    let validated_incumbent=curve.at(plan.qty).await.map_err(|e|e.to_string())?;
    let mut rank_ns=Vec::new();let mut fixed_ns=Vec::new();let mut search_ns=Vec::new();
    for _ in 0..31 {
        let t=Instant::now();std::hint::black_box(rank());rank_ns.push(t.elapsed().as_nanos() as u64);
        let t=Instant::now();std::hint::black_box(curve.at(plan.qty).await.unwrap());fixed_ns.push(t.elapsed().as_nanos() as u64);
        let t=Instant::now();std::hint::black_box(search(&mut curve,&p).await.unwrap());search_ns.push(t.elapsed().as_nanos() as u64);
    }
    let times=|mut ns:Vec<u64>|{ns.sort_unstable();json!({"median_ns":ns[15],"p95_ns":ns[29]})};
    let cmp=json!({"reconstructed_open_qty":reconstructed.best.qty,"archived_open_qty":plan.qty,
      "reconstructed_samples":memo.evals(),"successful_reconstructed_samples":old_points,
      "stored_points_with_incumbent":compact.len(),"compact_bytes":compact.len()*std::mem::size_of::<(Decimal,Decimal,Decimal,Decimal,Decimal,Decimal,Decimal)>(),
      "incumbent_profit":validated_incumbent.profit,"reranked_qty":pick,"reranked_profit":rerated,"fresh_validation_profit":fresh.profit,
      "max_rerating_error":max_diff,"rank_control":times(rank_ns),"one_fresh":times(fixed_ns),"full_native":times(search_ns)});
