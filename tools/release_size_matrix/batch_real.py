"""Stage existing historical drivers with private inputs; evaluate only the checked-out production core."""
import json,sys,pathlib,subprocess,re
from collections import Counter
from tempfile import TemporaryDirectory
p=pathlib.Path(__file__).resolve().parent
target=pathlib.Path(sys.argv[1]).resolve()
src=json.loads(pathlib.Path(sys.argv[2]).read_text())
out=pathlib.Path(sys.argv[3]);out.mkdir(exist_ok=True)
sha=subprocess.check_output(['git','rev-parse','HEAD'],cwd=target,text=True).strip()
def invoke(args):subprocess.run([sys.executable,*map(str,args)],check=True,stdout=subprocess.DEVNULL)
def cargo(names,expected,output):
 log=out/(output+'.log'); artifact=out/(str(sha)+'-'+output+'.json')
 with log.open('wb')as handle:
  res=subprocess.run(['cargo','test','--locked',*(v for n in names for v in ['--test',n]),'--','--nocapture'],cwd=target,stdout=handle,stderr=subprocess.STDOUT)
 text=log.read_text(errors='replace')
 if res.returncode:
  capture=[l[:260]for l in text.splitlines() if re.search(r'error(\[E[0-9]+\])?:|panicked at|FAILED|assertion',l)][:10]
  if output=='reconcile' and sha not in json.loads((p/'legacy_early_shas.json').read_bytes()):raise RuntimeError(capture)
  artifact.write_text(json.dumps({'compatibility_error':capture}));return
 records=[]
 for line in text.splitlines():
  m=re.search(r'(?:STORED_TRADE_MATRIX|ACTUAL_V2_MATRIX|ACTUAL_V3_MATRIX) (\{.*})$',line)
  if m:records.append(json.loads(m[1]))
 assert len(records)==expected,(len(records),expected,names)
 artifact.write_text(json.dumps(records));log.unlink()
def stage_hops(lane,module):
 data=src[lane];x=d/(lane+'.json');x.write_text(json.dumps(data))
 invoke([p/'general_archive.py',target,x,module])
 base=target/'tests';rs=(base/'general_archive.rs').read_text().replace('general_cases.json',lane+'_cases.json')
 name=lane+'_archive'
 if name!='general_archive':(base/'general_archive.rs').unlink()
 (base/(name+'.rs')).write_text(rs)
 (base/'general_cases.json').rename(base/(lane+'_cases.json')) if lane!='general' else None
 cargo([name],2*len(data['cases']),lane)
with TemporaryDirectory() as temp:
 d=pathlib.Path(temp); early=sha in json.loads((p/'legacy_early_shas.json').read_bytes())
 if early:
  settings=json.loads((p/'control_settings.json').read_text(encoding='utf-8-sig'))
  for lane,n in [('cex','stored'),('dex','actual_v2')]:
   data=src[lane];settings.update(data['settings']);(d/(lane+'.json')).write_text(json.dumps({'cases':data['cases']}))
  invoke([p/'stored_compare.py',target,d/'cex.json'])
  invoke([p/'actual_v2.py','stage',target,d/'dex.json'])
  invoke([p/'actual_v3.py','stage',target,p/'actual_v3_cases.json'])
  hist=d/'with_history.py';hist.write_bytes((p/'with_history.py').read_bytes());(d/'control_settings.json').write_text(json.dumps(settings));invoke([hist,target])
  cargo(['core_size_stored_compare','core_size_actual_v2','core_size_actual_v3'],2*(len(src['cex']['cases'])+len(src['dex']['cases'])+1),'core')
  stage_hops('general','single_tick_source.rs');stage_hops('tick','full_tick_source.rs')
 stage_hops('reconcile','reconcile_source.rs')
