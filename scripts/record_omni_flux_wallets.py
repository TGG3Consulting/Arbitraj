"""Проверяет каждую из найденных позиций Omni Flux CL через собственный UR и обычный V5."""
import argparse
import gzip
import json
from pathlib import Path
from record_base_launch_hooks import DATA, ENDPOINTS
from record_base_launch_wallets import replay, WETH, USDC, ZERO

def record(ep):
    with gzip.open(DATA/"omni_flux_base_directory.json.gz") as f:directory=json.load(f)
    snapshot=json.loads((DATA/"base_recovered_markets.json").read_bytes())
    bindings=json.loads((DATA/"omni_flux_roles.json").read_bytes())
    if not directory["selector"]==snapshot["selector"]==bindings["selector"]: raise ValueError("другой блок")
    r=bindings["roles"];group="omni-flux"
    cfg={"dex.universal_router":r["router"],"dex.permit2":r["permit2"],"dex.pool_manager":r["clPoolManager"],
         "dex.quoter":r["CLQuoter"],"dex.vault":r["vault"]}
    rows=[]
    for m in directory["markets"]:
        key=m["key"]
        cash=next((t for t in (WETH,USDC,ZERO) if t in key[:2]),key[0])
        rows.append(dict(id=m["id"],group=group,key=key,terms=dict(quote=cash)))
    framed=dict(selector=directory["selector"],state_root=snapshot["state_root"],height=snapshot["height"],
                manager=cfg["dex.pool_manager"],flux=cfg,rows=rows)
    return replay(group,ep,framed)
if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("output",type=Path);a=p.parse_args()
    for ep in ENDPOINTS:
        try:
            data=record(ep);a.output.write_text(json.dumps(data,indent=2)+"\n",encoding="utf8");break
        except (ValueError,OSError) as e:print(ep,str(e)[:250],flush=True)
    else:raise ValueError("архив не ответил")
