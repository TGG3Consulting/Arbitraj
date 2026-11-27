"""Проверяет фактический платёж на восстановленных рынках Base по опубликованному реестру."""
import argparse
import gzip
import json
from pathlib import Path
from record_base_launch_hooks import DATA,ENDPOINTS
from record_base_launch_wallets import replay
from record_base_algebra_markets import capture as algebra
from record_solidly_v3 import HOLDERS

def record(group,ep):
    d=json.loads((DATA/"base_recovered_markets.json").read_bytes())
    selector=d["selector"]
    if group=="treble-limit":
        with gzip.open(DATA/"treble_limit_epochs.json.gz") as stream: catalogue=json.load(stream)
        if catalogue["selector"]!=selector:raise ValueError("Treble limits refer to another block")
        markets=[]
        for row in catalogue["markets"]:
            if row["indexer_id"]!="treble-v4":raise ValueError("unidentified limit market")
            replayed=algebra("treble-v4",ep,(catalogue["factory"],row["pool"]),keep_refusals=True)
            if replayed["roles"]["limit_order_manager"]["address"]!=catalogue["manager"]:
                raise ValueError("the pool plugin uses a different order manager")
            markets.append(replayed)
        return dict(group=group,selector=selector,markets=markets)
    if group=="baseline":
        b=d["baseline"]
        directory=dict(selector=selector,state_root=d["state_root"],height=d["height"],manager=b["initialize"]["address"],
                       rows=[dict(id=b["pool_id"],key=b["key"],group=group,terms=dict(quote=b["key"][1]))])
        return replay(group,ep,directory)
    with gzip.open(DATA/"virtuals-base-protected.json.gz") as stream:v=json.load(stream)
    if v["selector"]!=selector:raise ValueError("иной блок исходного держателя VIRTUAL")
    owners=dict(HOLDERS["base"]);owners["0x0b3e328455c4059eeb9e3f84b5543f74e24e7e1b"]=v["owner"]
    directory=d["thirdfy"];markets=[]
    for row in directory["markets"]:
        pool=row["pool"]["address"]
        markets.append(algebra("thirdfy",ep,(directory["factory"]["address"],pool),owners))
    return dict(group=group,selector=selector,markets=markets)

if __name__=="__main__":
    args=argparse.ArgumentParser(description=__doc__);args.add_argument("group",choices=["thirdfy","baseline","treble-limit"]);args.add_argument("output",type=Path);a=args.parse_args()
    for ep in ENDPOINTS:
        try:
            r=record(a.group,ep);a.output.write_text(json.dumps(r,indent=2)+"\n",encoding="utf8");break
        except (OSError,ValueError,KeyError) as exc:print(ep,str(exc)[:250],flush=True)
    else:raise ValueError("нет проверенного архива")
