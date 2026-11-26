"""Exercise native versioned money and threshold functions at each archived size."""
import hashlib
import json
import os
import re
import subprocess
import sys
from collections import Counter
from decimal import Decimal, getcontext
from pathlib import Path

getcontext().prec = 70
src, data, results = map(Path, sys.argv[1:])
sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True, cwd=src).strip()
test = (Path(__file__).parent / 'fixed_archive.rs').read_text()
reserve_file=src/'src/core/reserve.rs'
start=test.index('    let keep=reserve::keep_of')
end=test.index('    let mut outgoing',start)
native_costs=(src/'src/core/costs.rs').read_text()
if reserve_file.exists():
    assert 'let reserve = super::reserve::amount(' in native_costs
    replacement='    let risk=reserve::amount(p.k,p.sigma,sqrt_t,quoted_proceeds,p.closure,p.qa[2],p.qb[2]);'
else:
    expr=re.search(r'let reserve = (cx\.k \* cx\.sigma \* sqrt_t \* proceeds);',native_costs)
    assert expr, 'unrecognized legacy reserve calculation'
    test=test.replace('Rates},reserve,types','Rates},types')
    start=test.index('    let keep=reserve::keep_of')
    end=test.index('    let mut outgoing',start)
    replacement='    let cx=&p; let proceeds=quoted_proceeds; let risk='+expr[1]+';'
test=test[:start]+'    use rust_decimal::MathematicalOps;\\n    let sqrt_t=p.transit.sqrt().ok_or("missing transit")?;\\n'+replacement+'\\n'+test[end:]
(src/'tests/fixed_archive.rs').write_text(test)
temp = Path(os.environ['RUNNER_TEMP'])
native_output = temp/'sha-points.json'
with (temp/'cargo.log').open('w') as f:
    ret = subprocess.run(['cargo', 'test', '--locked', '--test', 'fixed_archive'], cwd=src,
                         env=dict(os.environ, ARCHIVE_IN=str(data), ARCHIVE_OUT=str(native_output)),
                         stdout=f, stderr=subprocess.STDOUT)
if ret.returncode:
    lines=(temp/'cargo.log').read_text().splitlines()
    selected=set()
    for i,line in enumerate(lines):
        if re.search(r'error(\[E\d+\])?:|panicked at|assertion|FAILED',line):
            selected.update(range(i,min(i+5,len(lines))))
    for i in sorted(selected)[:75]:
        print(lines[i][:550])
    sys.exit(ret.returncode)
rows=json.loads(native_output.read_bytes())
assert len(rows)==12766
def tally(key, passed):
    a=[Decimal(r[key]) for r in rows if r.get(passed)]
    return {'passes':len(a), 'usdt':str(sum(a))}
r={'sha':sha,'total':len(rows),'present':sum('after' in row for row in rows),
   'reasons':dict(Counter(row['unavailable'] for row in rows if 'unavailable' in row)),
   'at_quote':tally('quoted','quoted_pass'),'final':tally('after','pass'),
   'row_digest':hashlib.sha256(json.dumps(rows,sort_keys=True,separators=(',',':')).encode()).hexdigest()}
(results/(sha+'.json')).write_text(json.dumps(r))
