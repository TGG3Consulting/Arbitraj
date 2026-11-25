from pathlib import Path
import sys
r=Path(sys.argv[1]);p=r/'tests/core_size_actual_v2.rs';s=p.read_text()
# Both product versions have a Rust field named contract and a persisted JSON field
# called bsc_contract. The archived JSON already uses the correct serde spelling.
assert '#[serde(rename = "bsc_contract")]' in (r/'src/core/types/market.rs').read_text()
start=s.index('fn convert_asset(');end=s.index('fn read_book(',start)
s=s[:start]+"""fn convert_asset(v: &mut Value) {
 if let Some(fields)=v.as_object_mut() && !fields.contains_key("bsc_contract") && let Some(addr)=fields.remove("contract"){
   fields.insert("bsc_contract".into(),addr);
 }
}
"""+s[end:]
a='let plan = Plan::parse(&plan)?;';assert s.count(a)==1
s=s.replace(a,a+' assert!(dir.asset.contract.is_some() && dir.asset.contract==plan.buy_quote.asset.contract && dir.asset.contract==plan.sell_quote.asset.contract,"preserve archived contract identity");')
p.write_text(s)
