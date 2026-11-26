mod historical_size {
    pub mod size;
    pub mod size_bounds;
    pub mod size_memo;
    pub mod size_search;
}
struct HistoricalCurve<'a, C>(&'a mut C);
#[async_trait::async_trait]
impl<C: Curve> historical_size::size::Curve for HistoricalCurve<'_, C> {
    type Data = C::Data;
    async fn at(&mut self,q:Decimal)->Result<historical_size::size::Point<Self::Data>,historical_size::size::Fail>{
        use arb::core::size::Fail as F;
        use historical_size::size::{Fail as H,Point};
        match self.0.at(q).await {
            Ok(v)=>Ok(Point{qty:v.qty,invested:v.invested,arrive:v.arrive,profit:v.profit,data:v.data}),
            Err(F::TooSmall(e))=>Err(H::TooSmall(e)),
            Err(F::TooLarge(e))=>Err(H::TooLarge(e)),
            Err(F::Stop(e))=>Err(H::Stop(e)),
            Err(F::OverBudget(e))=>Err(H::TooLarge(e)),
        }
    }
}
async fn historical_search<C: Curve>(c:&mut C,p:&Params)->Result<historical_size::size::Found<C::Data>,historical_size::size::Fail>{
    let args=historical_size::size::Params{
        step:p.step,budget:p.budget,guess:p.guess,buy_breaks:p.buy_breaks.clone(),
        sell_breaks:p.sell_breaks.clone(),bisect_steps:p.bisect_steps,grid_points:p.grid_points,
        refine_steps:p.refine_steps,tolerance_pct:p.tolerance_pct,max_evals:p.max_evals,
    };
    historical_size::size_search::search(&mut HistoricalCurve(c),&args).await
}
