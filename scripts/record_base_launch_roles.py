"""Читает размещения и условия каждого подтверждённого Base PoolId; перепроверяет источник bytecode."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode,encode
from eth_utils import keccak
from record_base_launch_hooks import DATA, ENDPOINTS, KEY

ZERO="0x"+"00"*20

class Capture:
    def __init__(self,snap):
        self.selector=snap["selector"];self.endpoint=ENDPOINTS[0];self.calls=[];self.cache={}
    def one(self,to,sig,args=(),vals=(),returns=()):
        data="0x"+(keccak(text=sig)[:4]+encode(args,vals)).hex()
        if (to,data) not in self.cache:
            failure=None
            for ep in ENDPOINTS:
                try:
                    request=dict(jsonrpc="2.0",id=1,method="eth_call",params=[dict(to=to,data=data),self.selector])
                    r=requests.post(ep,json=request,timeout=12);r.raise_for_status();result=r.json()
                    if "result" not in result:raise ValueError(str(result.get("error"))[:220])
                    self.cache[to,data]=result["result"];self.calls.append(dict(to=to,data=data,result=result["result"],signature=sig));break
                except (ValueError,requests.RequestException) as exc:failure=exc
            else:raise ValueError(f"{to} {sig}: {failure}")
        return decode(returns,bytes.fromhex(self.cache[to,data][2:]))

def record(directory,folder,out):
    d=json.loads(directory.read_bytes());snap=json.loads((DATA/"base.json").read_bytes())
    c=Capture(dict(snap,endpoint=ENDPOINTS[0]))
    roles={}
    def address(a):return a.lower()
    for row in d["rows"]:
        if "key" not in row:continue
        x,y,fee,spacing,hook=row["key"];pid=bytes.fromhex(row["id"][2:])
        src=json.loads((folder/(hook+"-source.json")).read_bytes())
        if hook not in roles:
            code=requests.post(c.endpoint,json=dict(method="eth_getCode",params=[hook,c.selector],id=1,jsonrpc="2.0"),timeout=15).json()["result"]
            if code!=src["runtimeBytecode"]["onchainBytecode"] or "0x"+keccak(bytes.fromhex(code[2:])).hex()!=row["hook_hash"]:raise ValueError("источник не совпадает с кодом на высоте")
            roles[hook]=dict(source=f"https://sourcify.dev/server/v2/contract/8453/{hook}?fields=all",name=src["compilation"]["name"],runtime=row["hook_hash"],match=src["runtimeMatch"],bytes=row["hook_bytes"])
        if c.one(hook,"poolManager()",returns=["address"])[0]!=d["manager"]:raise ValueError("другой PoolManager")
        r=roles[hook]
        def read(sig,args=(),vals=()):
            ab=next(a for a in src["abi"] if a.get("type")=="function" and a["name"]==sig)
            def typ(v):return "("+",".join(typ(y) for y in v["components"])+")"+v["type"][5:] if v["type"].startswith("tuple") else v["type"]
            return c.one(hook,sig+"("+",".join(args)+")",args,vals,[typ(v) for v in ab["outputs"]])
        if r["name"]=="DopplerHookInitializer":
            actual=read("getPoolKey",["bytes32"],[pid]);asset=next(t for t in (x,y) if t!="0x4200000000000000000000000000000000000006")
            state=read("getState",["address"],[asset])
            if list(actual)!=row["key"] or list(state[5])!=row["key"]:raise ValueError("неверный ключ/актив Doppler")
            row["terms"]=dict(asset=asset,quote=state[0],after_hook=state[2],state=state[4],flags=read("isDopplerHookEnabled",["address"],[state[2]])[0],airlock=read("airlock")[0])
        elif r["name"]=="IndexHook":
            stable="0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";weth="0x4200000000000000000000000000000000000006"
            index=next(t for t in (x,y) if t not in (stable,weth));keys=read("poolsOf",["address"],[index])
            factory=read("factory")[0]
            if tuple(row["key"]) not in keys or not c.one(factory,"isVault(address)",["address"],[index],["bool"])[0]:raise ValueError("не зарегистрирован индекс")
            row["terms"]=dict(index=index,factory=factory,pass_required=read("passRequired",["address"],[index])[0],rates=list(read("feeRatesOf",["address"],[index])[0]),registry=read("registry")[0])
        else:
            conf=read("poolConfig",["bytes32"],[pid])
            if not conf[0]:raise ValueError("LaunchHook не регистрировал рынок")
            row["terms"]=dict(factory=read("factory")[0],escrow=read("feeEscrow")[0],asset=x if conf[1] else y,quote=y if conf[1] else x,config=dict(zip((a["name"] for a in next(a for a in src["abi"] if a.get("name")=="poolConfig")["outputs"]),conf)))
    d["contracts"]=roles;d["calls"]=c.calls
    out.write_text(json.dumps(d,ensure_ascii=False,indent=2)+"\n",encoding="utf8")

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("directory",type=Path);ap.add_argument("sources",type=Path);ap.add_argument("output",type=Path)
    a=ap.parse_args();record(a.directory,a.sources,a.output)
