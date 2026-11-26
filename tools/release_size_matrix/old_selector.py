"""Run actual historical size search on a common archived monetary curve with fixed release accounting."""
import json,re,sys,subprocess,hashlib,tempfile
from pathlib import Path
t=Path(__file__).resolve().parent;root=Path(sys.argv[1]);d=Path(tempfile.mkdtemp());out=Path(sys.argv[3]);data=json.loads(Path(sys.argv[2]).read_bytes());cfg=json.loads((t/'control_settings.json').read_bytes())
def cmd(v):subprocess.run([sys.executable,*map(str,v)],stdout=subprocess.DEVNULL,check=True)
for key in ('cex','dex'):
 rows=data[key];cfg.update(rows['settings']);(d/(key+'.json')).write_text(json.dumps({'cases':rows['cases']}))
cmd([t/'stored_compare.py',root,d/'cex.json']);cmd([t/'actual_v2.py','stage',root,d/'dex.json']);cmd([t/'actual_v3.py','stage',root,t/'actual_v3_cases.json'])
(d/'with_history.py').write_bytes((t/'with_history.py').read_bytes());(d/'control_settings.json').write_text(json.dumps(cfg));cmd([d/'with_history.py',root])
tests=['core_size_stored_compare','core_size_actual_v2','core_size_actual_v3'];evidence=t/'historical_curve.inc'
for name in tests:
 f=root/'tests'/(name+'.rs');s=f.read_text();assert s.count('= search(&mut curve, &p).await;')==1,name
 f.write_text(s.replace('= search(&mut curve, &p).await;','= historical_search(&mut curve, &p).await;')+chr(10)+evidence.read_text())
files=['size','size_search','size_bounds','size_memo'];m=root/'tests/historical_size';m.mkdir()
def show(sha,path):return subprocess.check_output(['git','-C',str(root),'show',sha+':'+path])
need={'serde','serde_derive','async-trait','rust_decimal','num-traits','arrayvec','syn','quote','proc-macro2'}
import tomllib
def locked(sha):
 pkgs=tomllib.loads(show(sha,'Cargo.lock').decode())['package']
 return sorted((x['name'],x['version'],x.get('checksum'))for x in pkgs if x['name'] in need)
accounting=subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip();deps=locked(accounting)
for sha in sys.argv[4:]:
 assert locked(sha)==deps,('locked numeric dependencies differ',sha)
 fingerprint=hashlib.sha256()
 for mod in files:
  b=show(sha,'src/core/'+mod+'.rs');fingerprint.update(b);f=m/(mod+'.rs')
  if not f.exists() or f.read_bytes()!=b:f.write_bytes(b)
 with (d/'cargo.log').open('wb') as log:
  r=subprocess.run(['cargo','test','--locked',*(v for name in tests for v in ('--test',name)),'--','--nocapture'],cwd=root,stdout=log,stderr=subprocess.STDOUT)
 text=(d/'cargo.log').read_text(errors='replace')
 if r.returncode:
  print(chr(10).join(l[:240]for l in text.splitlines()if re.search(r'error(?:\[E[0-9]+\])?:|panicked at|FAILED|assertion',l))[:2000]);raise SystemExit(r.returncode)
 rows=[]
 for line in text.splitlines():
  match=re.search(r'(STORED_TRADE_MATRIX|ACTUAL_V2_MATRIX|ACTUAL_V3_MATRIX) (\{.*})$',line)
  if match:rows.append(json.loads(match[2]))
 assert len(rows)==2*(len(data['cex']['cases'])+len(data['dex']['cases'])+1)
 (out/(sha+'-selector.json')).write_text(json.dumps({'sha':sha,'market_costs_sha':accounting,'search_modules_fingerprint':fingerprint.hexdigest(),'tested':rows}))
