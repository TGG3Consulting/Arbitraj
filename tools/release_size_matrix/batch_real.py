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
 if sha in json.loads((p/'legacy_rate_string.json').read_bytes()):
  for name in names:
   test=target/'tests'/(name+'.rs');body=test.read_text()
   test.write_text(re.sub(r'Result<Rates,\s*Fail>', 'Result<Rates,String>',body))
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
 d=pathlib.Path(temp)
 stage_hops('intrablock','reconcile_source.rs')
