use arb::core::{convert::Rates, costs::evaluate_with, costs_source::Source, dirctx::DirCtx, size::Fail};
use arb::core::types::{Cost, CostKind, Direction, OrderBook, PriceLevel, VenueId};
use arb::core::uncapped_recompute_cfg::presence;
use arb::core::card_max::passes;
use arb::gateway::frozen::{BookFee, FrozenBook, deepest};
use chrono::Utc;

fn num(s: &str) -> Decimal { s.parse().unwrap() }

struct TwoLeg<'a> {
    parts: &'a Parts,
    book: FrozenBook,
}

impl Source for TwoLeg<'_> {
    async fn quote(&self, _: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        if sell {
            shopinx::full(&self.parts.quoting(), Side::Sell, qty).await
                .map_err(|e| Fail::TooLarge(format!("DEX {:?}", e.code)))
        } else {
            Ok(self.book.price(qty))
        }
    }
    fn worst(&self, _: &Direction, sell: bool, qty: Decimal, avg: Decimal) -> Result<Decimal,Fail> {
        if sell { return Ok(avg); }
        deepest(&self.book.book,Side::Buy,qty).ok_or_else(|| Fail::TooLarge("missing CEX depth".into()))
    }
    async fn withdraw(&self, _: &DirCtx, _: Decimal) -> Result<Vec<Cost>,Fail> {
        Ok(vec![Cost::new(CostKind::WithdrawFee, Decimal::ONE, "SHOPINX", false)])
    }
    async fn ret(&self, _: &DirCtx, _: Decimal) -> Result<Vec<Cost>,Fail> {
        Ok(vec![Cost::new(CostKind::TransferGas,num("0.000021"),"BNB",false)])
    }
    async fn rates(&self, cx: &DirCtx, coin:&str, coin_price:Decimal, _: &[Cost]) -> Result<Rates,Fail> {
        Ok(Rates{quote: cx.quote.clone(),coin:coin.to_string(),coin_price,
            prices:[("BNB".to_string(),num("598.53"))].into_iter().collect()})
    }
}

fn ctx(dir: &Direction,h:Decimal) -> DirCtx {
    let qa="USDT".to_string(); let qb=qa.clone();
    let k=num("3.6"); let sigma=num("0.0001"); let transit=num("60");
    let closure_share=Some(num("0.8"));
    // SHOP_CTX
}

#[tokio::test]
async fn shopinx_control() {
    let parts=shopinx::scene().await;
    let observed:Value=serde_json::from_str(include_str!("control_shop_book.json")).unwrap();
    let symbols:Value=serde_json::from_str(include_str!("control_shop_symbol.json")).unwrap();
    let rules=&symbols["data"]["symbols"][0]; let recorded=&observed["data"][0];
    let dir=Direction{asset:shopinx::shopinx(),buy_on:VenueId("bingx".into()),sell_on:VenueId("bsc_dex".into())};
    for mode in ["classic","graph"] {
        configure(&parts,mode).await;
        for (book_kind,price,depth,enforce_min,h) in [
            ("recorded_top_only",num(recorded["askPrice"].as_str().unwrap()),num(recorded["askVolume"].as_str().unwrap()),true,Decimal::ONE),
            ("recorded_top_only",num(recorded["askPrice"].as_str().unwrap()),num(recorded["askVolume"].as_str().unwrap()),false,Decimal::ONE),
            ("favourable_control",num("0.006"),num("100000"),true,Decimal::ONE),
            ("favourable_control",num("0.006"),num("100000"),false,Decimal::ONE),
            ("favourable_control",num("0.006"),num("100000"),false,num("0.7")),
        ] {
            let now=Utc::now(); let mut cx=ctx(&dir,h);
            let min=if enforce_min {rules["minQty"].clone()} else {Value::Null};
            let max=if enforce_min {rules["maxQty"].clone()} else {Value::Null};
            cx.pair_a=Some(serde_json::from_value(json!({"ticker":"SHOPINX","online":true,"status":1,
                "api_buy":true,"api_sell":true,"min_qty":min,"max_qty":max,
                "min_notional":rules["minNotional"],"max_notional":rules["maxNotional"],"step_size":rules["stepSize"]})).unwrap());
            let book=OrderBook{venue:dir.buy_on.clone(),asset:dir.asset.clone(),received_at:now,exchange_time:None,
                bids:vec![],asks:vec![PriceLevel{price,qty:depth}]};
            let source=TwoLeg{parts:&parts,book:FrozenBook{venue:dir.buy_on.clone(),asset:dir.asset.clone(),side:Side::Buy,
                book,fee:BookFee::in_received(num("0.001"),Side::Buy,"SHOPINX","USDT"),taken_at:now}};
            for gross in [1000,9000,19000,28500,29000,57000,57505] {
                let result=evaluate_with(&cx,Decimal::from(gross),&source).await;
                let data=match result {
                    Ok(p)=>json!({"passes":passes(&p,num("0.05"),Decimal::ZERO),"sell_qty":p.sell_qty,"profit_usdt":p.profit_usd,
                        "spent_usdt":p.invested,"proceeds_usdt":p.proceeds,"costs_usdt":p.costs_usdt,"costs":p.costs}),
                    Err(e)=>json!({"passes":false,"error":format!("{e:?}")})
                };
                println!("SHOP_TWO_LEG {}",json!({"router":mode,"book":book_kind,"buy_price":price,"buy_depth":depth,
                    "min_qty_enforced":enforce_min,"h":h,"buy_qty":gross,"block":shopinx::BLOCK,"result":data}));
            }
        }
    }
}
