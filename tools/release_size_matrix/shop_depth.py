from pathlib import Path
import sys
r=Path(sys.argv[1]); t=Path(__file__).parent
# Only external dependencies are mocked. Every comparison and routing operation stays native.
f=r/'src/venues/bsc_dex/quote/mod.rs'
s=f.read_text();start=s.index('let gas = self\n',s.index('async fn priced_at'));end=s.index('tm.add(Part::Gas',start)
s=s[:start]+'let gas = U256::from(50_000_000u64);\n        '+s[end:]
s=s.replace('approve_gas::units(self, &q, coin_dec, usdt_dec).await?', 'Option::<u64>::None')
f.write_text(s)
f=r/'src/venues/bsc_dex/quote/graph_split.rs';s=f.read_text();pat='    quote_timing::count(Count::SplitSteps, steps);'
s=s.replace(pat, '    if std::env::var_os("SHOP_TRACE").is_some() { println!("SHOP_SPLIT_TRACE qty={qty} unit={unit} left={left} step_count={steps} cands={:?}", cands.iter().map(|c|(c.qty,c.got.as_ref().map(|p|p.coins))).collect::<Vec<_>>()); }\n'+pat);f.write_text(s)
f=r/'tests/native_route_release_matrix.rs';f.write_text(f.read_text()+(t/'shop_depth.rs').read_text())
