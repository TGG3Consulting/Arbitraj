from pathlib import Path
import sys,json
p=Path(sys.argv[1])/'tests'
c=p/'size_actual_v2_cases.json'
x=json.loads(c.read_text())
src=Path(__file__).parent
e=json.loads((src/'risk_open_evidence.json').read_text(encoding='utf-8-sig'))
for v in x:v['open_evidence']=e[v['deal_id']]
c.write_text(json.dumps(x))
f=p/'core_size_actual_v2.rs';s=f.read_text()
mark='    let found = search(&mut curve, &p).await;'
assert s.count(mark)==1
w=(src/'controlled_rerating.rs').read_text(encoding='utf-8-sig').replace('let old_points=points.len();','let old_points=points.len(); let evals=memo.evals(); drop(memo);').replace('memo.evals(),"successful_', 'evals,"successful_').replace('(spend,q)<(s,q)', '(spend,qty)<(s,q)')
s=s.replace(mark,w+'\n'+mark).replace('"neighbors": neighbors','"rerating":cmp, "neighbors": neighbors').replace('for max_evals in [300, 5000]','for max_evals in [300]').replace('Err(reason) => json!({"unavailable": reason}),','Err(reason) => panic!("{reason}"),')
f.write_text(s)
