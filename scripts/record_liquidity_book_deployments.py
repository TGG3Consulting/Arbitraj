"""Сверка двух семейств Liquidity Book с кодом соответствующего выпуска, реестром и штатным getSwapOut."""
import argparse
import hashlib
import io
import re
import tarfile
from pathlib import Path

from eth_utils import keccak
import requests

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture
from record_curve_ng_deployments import code

SPECS = {
    "base": ("swapline-base","1297c3822f0605e643155c35948959c0a0d05e17","lb_v22"),
    "ethereum": ("traderjoe-v2-1-ethereum","8eeb42d73b5fe473f5b65d544d27827065a83e32","lb_v21_clone"),
}


def normalize(src):
    return re.sub(r"pragma solidity \^?0\.8\.10;", "pragma solidity 0.8.10;", src.replace("\r\n","\n")).strip()


def exclude_tree_remove(src):
    return re.sub(r"    function remove\(.*?\n    }","",src,flags=re.S)


def source_release(e,impl,rev):
    url=f"https://sourcify.dev/server/v2/contract/{e['chain_id']}/{impl}?fields=sources,compilation,runtimeBytecode.onchainBytecode"
    response=requests.get(url,timeout=20)
    response.raise_for_status()
    d=response.json()
    if "LBPair.sol:LBPair" not in d["compilation"]["fullyQualifiedName"] or not d["runtimeMatch"]:
        raise ValueError("реализация пула не подтверждена")
    upstream=f"https://codeload.github.com/lfj-gg/joe-v2/tar.gz/{rev}"
    archive=requests.get(upstream,timeout=20)
    archive.raise_for_status()
    with tarfile.open(fileobj=io.BytesIO(archive.content),mode="r:gz") as tar:
        src={m.name.split("/",1)[1]:tar.extractfile(m).read().decode() for m in tar.getmembers() if m.isfile() and m.name.endswith(".sol")}
    digest="0x"+keccak(bytes.fromhex(d["runtimeBytecode"]["onchainBytecode"][2:])).hex()
    if code(e,impl)!=digest:
        raise ValueError("код Sourcify и RPC расходятся")
    verified=[]
    for path,item in d["sources"].items():
        if not path.startswith("src/"):
            continue
        a=normalize(src[path])
        b=normalize(item["content"])
        if a!=b:
            # У варианта V2.1 изменено снятие пустого бина при burn; LBPair, поиск следующего бина,
            # расчёт комиссии и getSwapOut сравниваются целиком. Незнакомые изменения останавливают запись.
            if not (e["chain_id"]==1 and path=="src/libraries/math/TreeMath.sol"
                    and exclude_tree_remove(a)==exclude_tree_remove(b)):
                raise ValueError(f"расхождение с выпуском {path}")
            status="different_remove_on_burn"
        else:
            status="match"
        verified.append(dict(path=path,status=status,deployed_sha256=hashlib.sha256(b.encode()).hexdigest(),
                             upstream_sha256=hashlib.sha256(a.encode()).hexdigest()))
    return dict(source=url,upstream=upstream,compiler=d["compilation"]["compilerVersion"],runtime=digest,files=verified)


def market(c,pool,factory,impl,v22):
    x,=c.one(pool,"getTokenX()",returns=["address"])
    y,=c.one(pool,"getTokenY()",returns=["address"])
    step,=c.one(pool,"getBinStep()",returns=["uint16"])
    info=c.one(factory,"getLBPairInformation(address,address,uint256)",["address","address","uint256"],
               [x,y,step],["uint16","address","bool","bool"])
    f,=c.one(pool,"getFactory()",returns=["address"])
    if info[1]!=pool or info[0]!=step or f!=factory:
        raise ValueError("привязка фабрики или шага")
    r=requests.post(c.endpoint,json=dict(jsonrpc="2.0",id=1,method="eth_getCode",params=[pool,c.selector]),timeout=20)
    r.raise_for_status()
    raw=r.json()["result"]
    expected=("0x363d3d373d3d3d3d61002c806035363936013d73"+impl[2:]+"5af43d3d93803e603357fd5bf3"
              +x[2:]+y[2:]+step.to_bytes(2,"big").hex()+"002c")
    if raw!=expected:
        return dict(pool=pool,status="different_clone")
    if v22:
        address,=c.one(pool,"implementation()",returns=["address"])
        hook,=c.one(pool,"getLBHooksParameters()",returns=["bytes32"])
        if address!=impl or int.from_bytes(hook):
            return dict(pool=pool,status="hooks_or_different_implementation")
    reserves=c.one(pool,"getReserves()",returns=["uint128","uint128"])
    row=dict(pool=pool,x=x,y=y,step=step,clone=raw,reserves=list(map(str,reserves)),ignored_for_routing=info[3],quotes=[])
    if min(reserves)==0:
        return dict(**row,status="empty_side")
    for for_y,bal in zip((True,False),reserves):
        for divisor in (10000,100):
            amt=bal//divisor
            if not amt:
                continue
            left,out,fee=c.one(pool,"getSwapOut(uint128,bool)",["uint128","bool"],[amt,for_y],["uint128"]*3)
            row["quotes"].append(dict(swap_for_y=for_y,amount_in=str(amt),amount_left=str(left),amount_out=str(out),fee=str(fee)))
    ok=len(row["quotes"])==4 and all(q["amount_left"]=="0" and int(q["amount_out"])>0 for q in row["quotes"])
    row["status"]="quoted" if ok else "incomplete_fill"
    return row


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshots",type=Path)
    args=parser.parse_args()
    folder=ROOT/"tests/data/first_run_dex_2026-11-26"
    path=ROOT/"config/first_run_dex.json"
    for net,(dex,rev,kind) in SPECS.items():
        e=read(folder/f"{net}.json")
        c=Capture(e)
        samples=read(args.snapshots/f"{net}-integration-2026-11-26/sources/sample-pools.json")[dex]["samples"]
        candidates=[p["address"].lower() for p in samples]
        factory,=c.one(candidates[0],"getFactory()",returns=["address"])
        code(e,factory)
        impl,=c.one(factory,"getLBPairImplementation()",returns=["address"])
        release=source_release(e,impl,rev)
        n,=c.one(factory,"getNumberOfLBPairs()",returns=["uint256"])
        for i in range(min(n,12)):
            p,=c.one(factory,"getLBPairAtIndex(uint256)",["uint256"],[i],["address"])
            if p not in candidates:
                candidates.append(p)
        results=[]
        for p in candidates:
            results.append(market(c,p,factory,impl,kind=="lb_v22"))
            if sum(row["status"]=="quoted" for row in results)>=2:
                break
        if not any(r["status"]=="quoted" for r in results):
            raise ValueError(f"{net}: нет полной выборки рынка")
        write(folder/f"liquidity_book_{net}.json",dict(chain_id=e["chain_id"],endpoint=c.endpoint,selector=c.selector,
              factory=factory,implementation=impl,pairs=n,release=release,rows=results,calls=c.calls))
        write(folder/f"{net}.json",e)
        profiles=[r for r in read(path) if (r["network"],r["id"])!=(net,dex)]
        profiles.append(dict(chain_id=e["chain_id"],network=net,id=dex,source=release["source"],
                        settings={"dex.kind":kind,"dex.factory":factory,"dex.lb_implementations":[impl]}))
        write(path,profiles)
        print(net,n,[row["status"] for row in results],flush=True)


if __name__=="__main__":
    main()
