"""Проверяет каждую из найденных позиций Omni Flux CL через собственный UR и обычный V5."""
import argparse
import gzip
import json
from pathlib import Path
from record_base_launch_hooks import DATA, ENDPOINTS
from record_base_launch_wallets import replay, WETH, USDC, ZERO

def record(ep,bin_pool=False,wrapped_module=None,native_direct=False):
    source="omni_flux_registry.json.gz" if bin_pool else "omni_flux_base_directory.json.gz"
    with gzip.open(DATA/source) as f:directory=json.load(f)
    snapshot=json.loads((DATA/"base_recovered_markets.json").read_bytes())
    bindings=json.loads((DATA/"omni_flux_roles.json").read_bytes())
    if not directory["selector"]==snapshot["selector"]==bindings["selector"]: raise ValueError("другой блок")
    r=bindings["roles"];group="omni-flux-bin" if bin_pool else "omni-flux"
    cfg={"dex.universal_router":r["router"],"dex.permit2":r["permit2"],"dex.pool_manager":r["binPoolManager" if bin_pool else "clPoolManager"],
         "dex.quoter":r["BinQuoter" if bin_pool else "CLQuoter"],"dex.vault":r["vault"]}
    rows=[]
    for m in directory["markets"]:
        key=m["key"]
        if bin_pool:
            if m["kind"]!="bin":continue
            for i,cash in enumerate(key[:2]):
                quotes=[int(q["amount"]) for q in m["quotes"] if q["forward"]==(i==0) and q["out"] and int(q["out"])]
                sizes=sorted(set(quotes[-2:])) if quotes else ([1,10**6] if cash==USDC else [1,10**12])
                rows.append(dict(id=m["id"],group=group,key=key,terms=dict(quote=cash),sizes=sizes))
        else:
            cash=next((t for t in (WETH,USDC,ZERO) if t in key[:2]),key[0])
            rows.append(dict(id=m["id"],group=group,key=key,terms=dict(quote=cash)))
    framed=dict(selector=directory["selector"],state_root=snapshot["state_root"],height=snapshot["height"],
                manager=cfg["dex.pool_manager"],flux=cfg,rows=rows,native_direct=native_direct)
    if bin_pool:
        funding=json.loads((DATA/"omni_flux_funding.json").read_bytes())
        if funding["selector"]!=directory["selector"]:raise ValueError("держатели взяты с другого блока")
        framed["holders"]={x["token"]:x["owner"] for x in funding["holders"] if x.get("owner")}
    return replay(group,ep,framed,wrapped_module)
if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("output",type=Path);p.add_argument("--bin",action="store_true");a=p.parse_args()
    for ep in ENDPOINTS:
        try:
            data=record(ep,a.bin);a.output.write_text(json.dumps(data,indent=2)+"\n",encoding="utf8");break
        except (ValueError,OSError) as e:print(ep,str(e)[:250],flush=True)
    else:raise ValueError("архив не ответил")
