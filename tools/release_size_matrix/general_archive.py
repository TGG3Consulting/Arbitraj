"""Adapt only diagnostic test glue. Compile and exercise the actual checkout of each pinned release."""
from pathlib import Path
import json,tempfile,sys,re
import stored_compare
target=Path(sys.argv[1]);payload=json.loads(Path(sys.argv[2]).read_text());cases=payload['cases']
with tempfile.TemporaryDirectory()as d:
 p=Path(d);files=p/'cases.json';files.write_text(json.dumps({'cases':[{'basis':'cex_window_books'}]}));staged=stored_compare.stage(target,files);assert staged['status']=='staged',staged
out=target/'tests';s=(out/'core_size_stored_compare.rs').read_text();a=s.index('async fn replay(');b=s.index('#[tokio::test]',a)
s=s[:a]+(Path(__file__).parent/sys.argv[3]).read_text()+'\n'+s[b:]
s=s.replace('fn context(dir: &Direction, plan: &Plan) -> DirCtx {','fn context(dir: &Direction, plan: &Plan, k:Decimal,h:Decimal,closure_share:Option<Decimal>) -> DirCtx {').replace('    let (k, h, closure_share) = (d("3.6"), d("0.7"), Some(d("0.8")));','')
s=s.replace('STORED_TRADE_MATRIX','ACTUAL_V2_MATRIX').replace('include_str!("size_stored_cases.json")','include_str!("general_cases.json")')
s=s.replace('"model": {"k": "3.6", "h": "0.7", "closure_share": "0.8", "min_usdt": "0.05", "min_pct": "0", "eval_limit": max_evals}','"model": {"asof":case["control"], "eval_limit":max_evals}')
if 'pub fn swap_final(' not in (target/'src/venues/bsc_dex/math/swap_cl.rs').read_text() and 'swap_cl::swap_final' in s:
    s=s.replace('Version,After','Version').replace('fn uint(x:', '#[derive(Clone,Copy)] struct After {sqrt_price:U256,tick:i32,liquidity:u128}'+chr(10)+'fn uint(x:')
    a=s.index('let (swap,post)=swap_cl::swap_final');b=s.index('if exact_in && swap.amount_in',a)
    s=s[:a]+'if after.contains_key(id){return Err("historical swap API cannot advance a shared pool")}\n                let swap=swap_cl::swap(&state,*zin,exact_in,amount)?;\n                if !swap.complete{return Err("outside archive tick words")}\n                after.insert(id.clone(),current);\n                '+s[b:]
for c in cases:c["control"]=payload['settings'][c['deal_id']]
(out/'general_archive.rs').write_text(s);(out/'general_cases.json').write_text(json.dumps(cases));(out/'core_size_stored_compare.rs').unlink()
