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
  raise RuntimeError(capture)
  artifact.write_text(json.dumps({'compatibility_error':capture}));return
 records=[]
 for line in text.splitlines():
  m=re.search(r'(?:STORED_TRADE_MATRIX|ACTUAL_V2_MATRIX|ACTUAL_V3_MATRIX) (\{.*})$',line)
  if m:records.append(json.loads(m[1]))
 assert len(records)==expected,(len(records),expected,names)
 artifact.write_text(json.dumps(records));log.unlink()
with TemporaryDirectory() as temp:
 d=pathlib.Path(temp)
 # General requires pristine transformation of stored_compare; its generated Rust file is independent.
 for lane,module in [('general','single_tick_source.rs'),('tick','full_tick_source.rs')]:
  if not src.get(lane):continue
  x=d/(lane+'.json');x.write_text(json.dumps(src[lane]))
  invoke([p/'general_archive.py',target,x,module])
  if lane=='tick':
   srcfile=target/'tests/general_archive.rs'
   tt=srcfile.read_text();(target/'tests/tick_archive.rs').write_text(tt.replace('general_cases.json','tick_cases.json'))
   (target/'tests/general_cases.json').rename(target/'tests/tick_cases.json')
  else:
   (target/'tests/general_archive.rs').rename(d/'general.rs')
   (target/'tests/general_cases.json').rename(d/'general.json.native')
 if src.get('general'):
  (d/'general.rs').rename(target/'tests/general_archive.rs')
  (d/'general.json.native').rename(target/'tests/general_cases.json')
 if not src.get('tick'):raise ValueError("Missing route tick inputs")
 cargo(['tick_archive'],2*len(src['tick']['cases']),'tick')
