"""Архивные VirtualQuoter и ArbExecutor V5 на всех подтверждённых CREATE2 фабрики DerpDEX Base.
Для каждого рынка — обе стороны в исходном блоке, 4 величины входа в единицах актива,
затем покупка и продажа наблюдаемого выхода изолированным плательщиком.
"""
import json,sys
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import ROOT,OWNER,calldata,call,rpc
from record_pool_executor_controls import record
from record_metric_chains import VIRTUAL
from record_v2_chain_pools import view
from record_derpdex_base import FACTORY,FOLDER,WETH
from record_rubicon_clmm import pool_address

USDBC="0xd9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca"
def main(out):
    directory=json.loads((FOLDER/"derpdex_directory.json").read_bytes())
    old=json.loads((FOLDER/"derpdex_base.json").read_bytes())
    tag=directory["selector"]; init=bytes.fromhex(old["provenance"]["pool"]["actual"]["init_hash"][2:])
    virtual=(ROOT/"contracts/out/VirtualQuoter.bin-runtime").read_text().strip().removeprefix("0x")
    candidates=["https://base-rpc.publicnode.com","https://base.drpc.org","https://mainnet.base.org"]
    for url in candidates:
        try:
            if int(rpc(url,"eth_chainId",[]),16)!=8453:continue
            if rpc(url,"eth_getCode",[FACTORY,tag])=="0x":continue
            base={"code":"0x"+virtual}
            initial=rpc(url,"eth_simulateV1",[{"blockStateCalls":[{"stateOverrides":{VIRTUAL:base},"calls":[call(WETH,calldata("decimals()"))]}]},tag])
            if int(initial[0]["calls"][0]["returnData"],16)==18:break
        except Exception:pass
    else:raise ValueError("Нет ответившего архивного узла с eth_simulateV1")
    def read(a,s,types=(),args=()):return view(url,tag,a,calldata(s,types,args))
    def num(a,s):return int.from_bytes(read(a,s),"big")
    tokens={a for p in directory["pools"] for a in p["tokens"]}
    decimals={a:num(a,"decimals()") for a in tokens}
    def sizes(a):
        if a==WETH:return [10**12,10**14,10**16,10**17]
        unit=10**decimals[a]
        return [max(1,unit//1000),max(1,unit//10),unit*10,unit*100]
    def static(p):
        ts=p["tokens"];fee=p["fee"];addr=p["pool"]
        assert pool_address(FACTORY,ts,fee,init)==addr
        for path in [ts,list(reversed(ts))]:
            assert decode(["address"],read(FACTORY,"getPool(address,address,uint24)",["address","address","uint24"],[*path,fee]))[0]==addr
        actual=[decode(["address"],read(addr,s))[0] for s in ["token0()","token1()","factory()"]]
        if actual!=[*ts,FACTORY] or num(addr,"fee()")!=fee or num(addr,"tickSpacing()")!=p["spacing"]:
            raise ValueError("поля не прошли проверку: "+addr)
        plan=[(i,amount) for i,a in enumerate(ts) for amount in sizes(a)]
        probes=[call(VIRTUAL,calldata("quoteExactIn(address,bool,uint256)",["address","bool","uint256"],[addr,i==0,n])) for i,n in plan]
        resp=rpc(url,"eth_simulateV1",[{"blockStateCalls":[{"stateOverrides":{VIRTUAL:{"code":"0x"+virtual}},"calls":probes}]},tag])[0]["calls"]
        quotes=[]
        for (side,n),r in zip(plan,resp,strict=True):
            q={"side":side,"requested":str(n),"status":r["status"],"returnData":r.get("returnData")}
            if int(r["status"],16)==1:
                used,received=decode(["uint256","uint256"],bytes.fromhex(r["returnData"][2:]))
                q.update(used=str(used),out=str(received),full=(used==n and received>0))
            else:q["reason"]=r.get("error")
            quotes.append(q)
        return {**p,"liquidity":str(num(addr,"liquidity()")),"quotes":quotes}
    with ThreadPoolExecutor(max_workers=2) as pool:
        markets=list(pool.map(static,directory["pools"]))
    bridges={}
    for m in markets:
        if WETH not in m["tokens"]:continue
        asset=next(a for a in m["tokens"] if a!=WETH)
        if asset not in bridges or int(m["liquidity"])>int(bridges[asset]["liquidity"]):bridges[asset]=m
    def wallet(item):
        index,m=item;ts=m["tokens"]
        side=ts.index(WETH) if WETH in ts else ts.index(USDBC) if USDBC in ts else next((i for i,a in enumerate(ts) if a in bridges),0)
        funding=bridges.get(ts[side],{}) if ts[side]!=WETH else {}
        prepared=dict(idx=index,pool=m["pool"],tokens=ts,quotes=[{"side":side,"input":str(x)} for x in sizes(ts[side])],
                      funding_weth=10**18//4,funding_pool=funding.get("pool"))
        replay=record(dict(kind="v3",network="base",endpoint=url,hash=tag["blockHash"],block=directory["to"],contracts={"factory":FACTORY},
                       quoter_runtime_keccak="0x"+keccak(bytes.fromhex(virtual)).hex(),capture_partial=True,pools=[prepared]))
        m["funding_pool"]=prepared["funding_pool"];m["wallet"]=replay["pools"][0]["cases"]
        return m
    with ThreadPoolExecutor(max_workers=2) as pool:
        markets=list(pool.map(wallet,enumerate(markets)))
    result={"chain_id":8453,"selector":tag,"height":directory["to"],"endpoint":url,"sender":OWNER,
            "factory":FACTORY,"decimals":decimals,"markets":markets,"index":[x["pool"] for x in directory["indexed"]]}
    Path(out).write_bytes((json.dumps(result,indent=2)+"\n").encode())
    if len(markets)!=13 or len(result["index"])!=6 or not all(len(m["quotes"])==8 and len(m["wallet"])==4 for m in markets):
        raise ValueError("неполная таблица рынков/размеров")
    reference=next(m for m in markets if m["pool"]==old["provenance"]["pool"]["address"])
    if not all("sold" in row for row in reference["wallet"]):raise ValueError("прежде исполненный рынок не воспроизведён")

if __name__=="__main__":main(sys.argv[1])
