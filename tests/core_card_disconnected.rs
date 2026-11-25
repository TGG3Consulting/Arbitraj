use arb::core::size::{Curve, Fail, Params, Point};
use arb::core::size_max::max_volume;
use async_trait::async_trait;
use rust_decimal::Decimal as D;

#[derive(Default)]
struct Disconnected { seen: Vec<D> }
#[async_trait]
impl Curve for Disconnected {
    type Data = ();
    async fn at(&mut self, qty:D) -> Result<Point<()>,Fail> {
        self.seen.push(qty);
        if qty == D::from(6) || qty >= D::from(11) {
            return Err(Fail::TooLarge("cached route has no complete quote".into()));
        }
        Ok(Point{qty, invested:qty, arrive:qty, profit:qty, data:()})
    }
}
#[tokio::test]
async fn display_bound() {
    let mut curve=Disconnected::default();
    let p=Params {step:D::ONE, budget:D::from(100), guess:D::from(12),buy_breaks:vec![],sell_breaks:vec![],bisect_steps:12,grid_points:4,refine_steps:3,tolerance_pct:D::ONE,max_evals:300,par_points:4,bisect_probes:3,early_floor:None,reuse:false,par_blocked:false,ext:Default::default()};
    let r=max_volume(&mut curve,&p,D::ONE, |_|true).await.unwrap();
    assert!(curve.seen.contains(&D::from(9)));
    assert_eq!(r.opt_qty,D::from(9));
    assert_eq!(r.max_qty,Some(D::from(9)));
    assert_eq!(r.max_profit, Some(D::from(9)));
    assert_eq!(r.max_spend, Some(D::from(9)));
    assert_eq!(r.upper_qty, D::from(5));
    assert_eq!(curve.seen, [12, 3, 6, 9, 4, 5, 1, 2, 6].map(D::from));
}
