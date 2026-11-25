/// Exact left edge of a two-adjacent-buy-steps/same-sale-quantity plateau for
/// the known linear transfer in this test. No oracle or unseen price is accessible here.
fn plateau_trial(
    s: &Scenario, floor: (Decimal,Decimal), seen: &[(Decimal,Result<Sample,arb::core::size::Fail>)],
    endpoints: Option<(Decimal,Decimal,Decimal)>, incumbent: Option<&Sample>, limit: u32,
) -> (Option<Sample>, usize, usize) {
    use std::collections::BTreeSet;
    let mut best=incumbent.cloned();
    let eligible=matches!(s.buy,Market::Router(_)) && matches!(s.sell,Market::Ladder(_)) &&
        s.arrive_keep>Decimal::ZERO && s.arrive_keep<Decimal::ONE;
    let left=(limit as usize).saturating_sub(seen.len()).min(24);
    let Some((low,high,anchor))=endpoints.filter(|_|eligible && left>0) else{return(best,0,0)};
    let lo=(low/s.step).ceil();let hi=(high/s.step).floor();if hi<=lo{return(best,0,0)}
    let gap=Decimal::ONE-s.arrive_keep;let intercept=s.withdraw*s.arrive_keep/s.step;
    let first=(lo*gap+intercept).ceil();let last=((hi+Decimal::ONE)*gap+intercept).ceil()-Decimal::ONE;
    if last<first{return(best,0,0)}
    let center=((anchor/s.step)*gap+intercept).round().max(first).min(last);
    let previous:BTreeSet<_>=seen.iter().map(|(q,_)|*q).collect();
    let mut tried=BTreeSet::new();let mut probe=0;let mut priced=0;
    // Bound candidate generation even if most are already in the memo.
    for radius in 0..(2*24+1) {
        let dist=Decimal::from(radius);
        for edge in [center-dist,center+dist] {
            if edge<first || edge>last{continue}
            let n=((edge-intercept)/gap).floor();let q=n*s.step;
            if n<lo || n>=hi || !tried.insert(q){continue}
            probe+=1;
            if previous.contains(&q){continue}
            let sales=|x:Decimal|((x-s.withdraw)*s.arrive_keep/s.step).floor();
            if sales(q)!=sales(q+s.step){continue}
            priced+=1;
            if let Ok(p)=s.point(q) && passes(&p,floor) && best.as_ref().is_none_or(|b|rank(&p)>rank(b)) {best=Some(p);}
            if priced==left{return(best,priced,probe)}
        }
        if center-dist<=first && center+dist>=last {break}
    }
    (best,priced,probe)
}
