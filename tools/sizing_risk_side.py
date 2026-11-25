"""The closed-card comparator has no venue rule flags. Restore its observed CEX buy side
for the archived fast check. Historical order-size grid is the comparator's saved data
grid, not a claim that a complete historical symbol filter was captured."""
from pathlib import Path
import sys
p=Path(sys.argv[1])/'tests/core_size_actual_v2.rs';s=p.read_text()
a='let cx = context(&dir, &plan, decimal(&cfg["profit.price_move_k"])?, decimal(&cfg["profit.liquidity_share_h"])?);'
assert s.count(a)==1
s=s.replace(a,a.replace('let cx','let mut cx')+'''
    assert!(matches!((dir.asset.ticker.as_str(),dir.buy_on.0.as_str(),dir.sell_on.0.as_str()),("DEOD","poloniex","bsc_dex")|("RT","mexc","bsc_dex")|("DEOD","bingx","bsc_dex")|("REAL","bingx","bsc_dex")));
    cx.pair_a=Some(serde_json::from_value(json!({"ticker":dir.asset.ticker,"online":true,"status":1,"api_buy":true,"api_sell":true,"step_size":step_of(&plan,&a)})).unwrap());
''')
a='    let mut old_curve = SourceCurve { cx: &old_ctx, src: &source };';assert s.count(a)==1
p.write_text(s.replace(a,'    old_ctx.pair_a=cx.pair_a.clone();\n'+a))

# Feed the captured original and final CEX receive times into the cache in archival order.
f=Path(sys.argv[1])/'src/core/precheck_hint.rs'
h=f.read_text()
a='book:&crate::core::types::OrderBook, memo:'
assert h.count(a)==1
h=h.replace(a,'book:&crate::core::types::OrderBook, initial_book:Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>, memo:')
a='core.inner.market.write().unwrap_or_else(|p|p.into_inner()).apply(crate::gateway::events::MarketEvent::Book(book.clone()));'
assert h.count(a)==1
h=h.replace(a,'let mut old_book=book.clone();if let Some((t,_))=initial_book {old_book.received_at=t;}\n core.inner.market.write().unwrap_or_else(|p|p.into_inner()).apply(crate::gateway::events::MarketEvent::Book(old_book));')
a='remember(core,old,mark,memo,found);';h=h.replace(a,a+'\n let mut final_book=book.clone();if let Some((_,t))=initial_book {final_book.received_at=t;}\n core.inner.market.write().unwrap_or_else(|p|p.into_inner()).apply(crate::gateway::events::MarketEvent::Book(final_book));')
f.write_text(h)
f=Path(sys.argv[1])/'tests/core_size_actual_v2.rs';h=f.read_text();a='&cx,&a,&memo,&reconstructed';assert h.count(a)==2
h=h.replace(a,'&cx,&a,Some((earlier["book_stamp"].as_str().unwrap().parse().unwrap(),earlier["final_book_stamp"].as_str().unwrap().parse().unwrap())),&memo,&reconstructed');f.write_text(h)
f=Path(sys.argv[1])/'tests/native_route_release_matrix.rs';s=f.read_text().replace('&src.book.book,&m,found','&src.book.book,None,&m,found');f.write_text(s)
