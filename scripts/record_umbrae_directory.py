"""Подтверждает обе фабрики Umbrae, действующие реализации прокси и полный реестр по одному хешу Base."""
import argparse
import hashlib
import json
from pathlib import Path
import requests
from bs4 import BeautifulSoup
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT, rpc
from record_sectorone_directory import BASE_ENDPOINTS

DOC="https://www.umbrae.io/dev"
REG="https://www.umbrae.io/docs/dev/contracts.json"
ZERO="0x"+"00"*20
DATA=ROOT/"tests/data/first_run_dex_2026-11-26"

def directory(endpoint):
    snap=json.loads((DATA/"base.json").read_bytes())
    c=Capture(dict(snap,endpoint=endpoint));tag=c.selector
    header=rpc(endpoint,"eth_getBlockByHash",[tag["blockHash"],False])
    if int(header["number"],16)!=snap["block"] or int(rpc(endpoint,"eth_chainId",[]),16)!=8453:raise ValueError("неверная высота или сеть")
    doc=requests.get(DOC,timeout=25);doc.raise_for_status()
    reg=requests.get(REG,timeout=25);reg.raise_for_status();registry=reg.json()
    contracts={k:v.lower() for k,v in registry["contracts"].items()}
    rows=BeautifulSoup(doc.content,"html.parser").select("tr")
    addr={}
    for r in rows:
        cells=r.select("td");link=cells[1].select_one("a") if len(cells)>1 else None
        if link and len(link.get_text(strip=True))==42 and link.get_text(strip=True).startswith("0x"):addr[cells[0].get_text(" ",strip=True)]=link.get_text(strip=True).lower()
    scalar=lambda a,s,rt:c.one(a,s,returns=[rt])[0]
    damm=next(a for title,a in addr.items() if title.startswith("DAMMFactory (proxy"))
    raw_roles={"dlmmFactory":contracts["factory"],"dlmmRouter":contracts["router"],"dlmmViewer":contracts["pairViewer"],
               "dlmmBeacon":contracts["beacon"],"dlmmExtension":contracts["pairExtension"],"dammFactory":damm,
               "dammBeacon":addr["DAMMPairBeacon"],"dammRouter":addr["DAMMRouter"],"feeRouter":registry["swap"]["feeRouter"].lower()}
    def raw(method,params):
        r=c.session.post(endpoint,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=25);r.raise_for_status();return r.json()["result"]
    def code(address):
        value=bytes.fromhex(raw("eth_getCode",[address,tag])[2:])
        if not value:raise ValueError("пустая роль "+address)
        return {"address":address,"bytes":len(value),"hash":"0x"+keccak(value).hex()}
    def pointer(address,kind):
        slot=hex(int.from_bytes(keccak(text="eip1967.proxy."+kind))-1)
        val=raw("eth_getStorageAt",[address,slot,tag])
        if int(val[2:-40],16):raise ValueError("неверная ширина роли")
        return "0x"+val[-40:]
    roles={}
    for name,a in raw_roles.items():
        role=code(a);ip=pointer(a,"implementation")
        if ip!=ZERO:role["implementation"]=code(ip)
        if name.endswith("Beacon"):role["implementation"]=code(scalar(a,"implementation()","address"))
        actual=role.get("implementation",role)
        if name not in ("dlmmBeacon","dammBeacon"):
            url=f"https://sourcify.dev/server/v2/contract/8453/{actual['address']}"
            resp=requests.get(url,params={"fields":"abi,compilation,runtimeBytecode.onchainBytecode"},timeout=30);resp.raise_for_status();src=resp.json()
            rb=bytes.fromhex(src["runtimeBytecode"]["onchainBytecode"][2:])
            if src.get("runtimeMatch") not in ("match","exact_match") or "0x"+keccak(rb).hex()!=actual["hash"]:raise ValueError("расхождение исходника "+name)
            role["source"]=dict(url=url,name=src["compilation"]["name"],match=src["runtimeMatch"],abi_sha256=hashlib.sha256(json.dumps(src["abi"],sort_keys=True).encode()).hexdigest())
        roles[name]=role
    markets={}
    for family in ("dlmm","damm"):
        bins=family=="dlmm";f=roles[family+"Factory"]["address"];beacon=roles[family+"Beacon"]["address"]
        if scalar(f,"beacon()","address")!=beacon:raise ValueError("другая фабрика beacon")
        if bins and scalar(roles["dlmmRouter"]["address"],"FACTORY()","address")!=f:raise ValueError("роутер другой фабрики")
        n=scalar(f,"allPairsLength()","uint256")
        ps=[decode(["address"],v)[0] for v in posts(c,[(f,"allPairs(uint256)",["uint256"],[i]) for i in range(n)])]
        if len(set(ps))!=n:raise ValueError("повтор в реестре")
        result=[]
        for i,p in enumerate(ps):
            row=dict(index=i,pool=p,runtime=code(p),beacon=pointer(p,"beacon"))
            if row["beacon"]!=beacon:raise ValueError("другой beacon у пары")
            if bins:
                types=["address","address","address","uint16","uint24"]+["uint128"]*5
                metadata=c.one(f,"getPairMetadata(address)",["address"],[p],types)
                if metadata[0]!=p:raise ValueError("неверное имя пары")
                x,y=metadata[1:3];step=metadata[3];factor=c.one(f,"getPairBaseFactor(address)",["address"],[p],["uint16"])[0]
                args=["address","address","uint16","uint16"];val=[x,y,step,factor];row.update(step=step,base_factor=factor,active_id=metadata[4])
                rx,ry=c.one(p,"getReserves()",returns=["uint128","uint128"])
            else:
                x,y=[scalar(p,k+"()","address") for k in ("tokenX","tokenY")]
                args=["address","address"];val=[x,y]
                rx,ry=c.one(p,"getReserves()",returns=["uint112","uint112"])
                row.update(fee_token=scalar(p,"feeToken()","address"),fee=scalar(p,"currentFeeBps()","uint16"))
            regsig="getPair("+",".join(args)+")"
            for xx,yy in [(x,y),(y,x)]:
                if c.one(f,regsig,args,[xx,yy,*val[2:]],["address"])[0]!=p:raise ValueError("чужой рынок "+p)
            vals=posts(c,[(a,"balanceOf(address)",["address"],[p]) for a in [x,y]])
            result.append(dict(**row,tokens=[x,y],reserves=[str(rx),str(ry)],balances=[str(int.from_bytes(v)) for v in vals]))
        markets[family]=result
    return dict(chain_id=8453,height=snap["block"],selector=tag,state_root=header["stateRoot"],source=[DOC,REG],roles=roles,markets=markets,calls=c.calls)

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path);args=ap.parse_args()
    for e in BASE_ENDPOINTS:
        try:
            d=directory(e);args.output.write_text(json.dumps(d,indent=2)+"\n",encoding="utf8");break
        except (KeyError,ValueError,OSError) as exc:print(e,str(exc)[:320],flush=True)
    else:raise ValueError("реестр Umbrae не подтвердился")
