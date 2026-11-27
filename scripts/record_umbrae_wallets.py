"""Измеряет выплаты обоих Umbrae одним обычным кошельком в изолированной симуляции выбранного блока."""
import argparse
import json
from pathlib import Path
import requests
from eth_abi import encode,decode
from eth_utils import keccak
from record_base_slipstream_executor import EXEC,RUNTIME,STEP,calldata,rpc
from record_sectorone_directory import BASE_ENDPOINTS
from record_woo_base import WETH,HOLDERS
from record_umbrae_directory import DATA

MODULE="0x"+keccak(text="Arbitraj Base Umbrae recipient protected module Nov 2026")[-20:].hex()
word=lambda n:"0x"+format(n,"064x")

def record(d,family,artifact,endpoint):
    bins=family=="dlmm";tag=d["selector"];roles=d["roles"]
    router=roles["dlmmRouter"]["address"];viewer=roles["dlmmViewer"]["address"]
    all_markets=d["markets"];head=rpc(endpoint,"eth_getBlockByHash",[tag["blockHash"],False]);deadline=int(head["timestamp"],16)+120
    if head["stateRoot"]!=d["state_root"]:raise ValueError("изменилась опорная высота")
    roles4=[roles[k+"Factory"]["address"] for k in ("dlmm","damm")]+[all_markets[k][0]["pool"] for k in ("dlmm","damm")]
    creation={"from":HOLDERS[WETH],"data":"0x"+artifact["creation"]+encode(["address"]*4,roles4).hex()}
    def raw(method,params):
        if method not in ("eth_call","eth_estimateGas"):raise ValueError(method)
        r=requests.post(endpoint,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=35);r.raise_for_status();res=r.json()
        if "result" not in res:raise ValueError(str(res.get("error"))[:320])
        return res["result"]
    runtime=raw("eth_call",[creation,tag])
    built=bytes.fromhex(runtime[2:]);template=bytes.fromhex(artifact["runtime"]);check=bytearray(built)
    for refs in artifact["immutableReferences"].values():
        if len({built[r["start"]:r["start"]+r["length"]] for r in refs})!=1:raise ValueError("неравные копии immutable")
        for r in refs:
            a,n=r["start"],r["length"]
            if n!=32:raise ValueError("ширина immutable")
            check[a:a+n]=template[a:a+n]
    if bytes(check)!=template:raise ValueError("код отличается от сборки")
    gas=raw("eth_estimateGas",[creation,tag]);module_hash="0x"+keccak(built).hex()
    if any(rpc(endpoint,"eth_getCode",[a,tag])!="0x" for a in (MODULE,EXEC)):raise ValueError("адрес симуляции занят")
    def tx(w,to,sig,types=(),vals=()):
        return {"from":w,"to":to,"data":calldata(sig,types,vals),"gas":"0x700000"}
    def run(w,calls,mode):
        frame={"calls":calls}
        if mode!="native":
            frame["stateOverrides"]={MODULE:{"code":runtime}}
            if mode=="v5":
                frame["stateOverrides"][EXEC]={"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                                                "stateDiff":{word(0):word(int(w,16)),word(1):word(int(WETH,16))}}
        block,=rpc(endpoint,"eth_simulateV1",[{"blockStateCalls":[frame],"validation":False},tag])
        if block["parentHash"]!=tag["blockHash"] or len(block["calls"])!=len(calls):raise ValueError("неполная симуляция")
        return block["calls"]
    def leg(m,owner,prefix,mode,x,y,amount):
        pool=m["pool"];forward=x==m["tokens"][0]
        target=(router if bins else pool) if mode=="native" else (MODULE if mode=="module" else EXEC)
        approval=tx(owner,x,"approve(address,uint256)",["address","uint256"],[target,amount])
        balances=[tx(owner,t,"balanceOf(address)",["address"],[a]) for a in [owner,MODULE,EXEC] for t in (x,y)]
        permits=[tx(owner,x,"allowance(address,address)",["address","address"],[MODULE,pool])]
        quoted=(tx(owner,viewer,"quoteSwapDetailed(address,bool,uint256)",["address","bool","uint256"],[pool,forward,amount])
                if bins else tx(owner,pool,"getAmountOut(uint256,bool)",["uint256","bool"],[amount,forward]))
        pre=prefix+[approval,*balances,*permits,quoted];before=run(owner,pre,mode)
        if any(r["status"]!="0x1" for r in before):return None,dict(reason="исходный вызов отклонился",input=str(amount),calls=pre,results=before)
        qbytes=bytes.fromhex(before[-1]["returnData"][2:])
        q=decode(["uint256","uint256","uint24","uint256","uint256","bool"],qbytes) if bins else (int.from_bytes(qbytes),amount)
        net,used=q[:2]
        def order(floor,expiry):
            if mode=="v5":
                encoded=encode(["bytes32"],[int(pool,16).to_bytes(32)])
                return tx(owner,EXEC,f"execute(address,uint256,address,uint256,uint256,{STEP}[])",
                          ["address","uint256","address","uint256","uint256",STEP+"[]"],[x,amount,y,floor,expiry,[(17,MODULE,x,y,10000,encoded)]])
            if mode=="module":
                return tx(owner,MODULE,"execute(address,address,address,uint256,uint256,uint256)",["address","address","address","uint256","uint256","uint256"],[pool,x,y,amount,floor,expiry])
            if bins:
                ts=["uint256","uint256","address[]","uint16[]","address[]","address","uint256"]
                return tx(owner,router,"swapExactTokensForTokens("+",".join(ts)+")",ts,[amount,floor,[x,y],[m["step"]],[pool],owner,expiry])
            return tx(owner,pool,"swap(uint256,bool,uint256,address)",["uint256","bool","uint256","address"],[amount,forward,floor,owner])
        checks=[order(net+1,deadline)]
        if mode!="native" or bins:checks.append(order(max(1,net),0))
        exact=order(max(net,1),deadline);calls=pre+checks+[exact,*balances,*permits];results=run(owner,calls,mode)
        i=len(pre)+len(checks);fill=results[i];before_vals=[int(r["returnData"],16) for r in before[-8:-1]]
        after_vals=[int(r["returnData"],16) for r in results[i+1:]]
        if any(r["status"]!="0x0" for r in results[len(pre):i]):raise ValueError("минимум или срок нарушен "+pool)
        data=dict(input=str(amount),quoted=str(net),consumed=str(used),quote=[str(v) for v in q],calls=calls,results=results)
        if fill["status"]!="0x1":
            if after_vals!=before_vals:raise ValueError("откат изменил остаток")
            return None,dict(**data,reason="обмен отклонился")
        spent=before_vals[0]-after_vals[0];got=after_vals[1]-before_vals[1]
        if used!=amount or spent!=amount or got!=net or after_vals[2:]!=before_vals[2:] or after_vals[-1]:raise ValueError("расхождение дельт "+pool)
        return prefix+[approval,exact],dict(**data,paid=str(spent),net=str(got),gas=int(fill["gasUsed"],16))
    examples=[]
    for m in all_markets[family]:
        cash=next((t for t in [WETH,*HOLDERS] if t in m["tokens"]),None)
        if cash is None:examples.append(dict(pool=m["pool"],unresolved="нет подтверждённого держателя входного актива"));continue
        owner=HOLDERS[cash];y=next(t for t in m["tokens"] if t!=cash)
        if rpc(endpoint,"eth_getCode",[owner,tag])!="0x":raise ValueError("держатель не является EOA")
        sizes=([10**12,10**14] if cash==WETH else [100,10000])
        if y=="0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf" and cash==WETH:sizes=[10**13,10**14]
        cases=[]
        for qty in sizes:
            for mode in ("native","module","v5"):
                prefix,buy=leg(m,owner,[],mode,cash,y,qty)
                sell=leg(m,owner,prefix,mode,y,cash,int(buy["net"]))[1] if prefix else None
                cases.append(dict(mode=mode,amount=str(qty),buy=buy,sell=sell))
        examples.append(dict(pool=m["pool"],cash=cash,owner=owner,cases=cases))
    if not any(c["sell"] and "net" in c["sell"] for m in examples for c in m.get("cases",[]) if c["mode"]=="v5"):raise ValueError("нет защищённого кругового обмена")
    return dict(family=family,selector=tag,height=d["height"],constructor=roles4,module_hash=module_hash,deployment_gas=int(gas,16),examples=examples)

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("family",choices=("dlmm","damm"));ap.add_argument("artifact",type=Path);ap.add_argument("output",type=Path);args=ap.parse_args()
    artifact=json.loads(args.artifact.read_bytes());directory=json.loads((DATA/"umbrae_base_directory.json").read_bytes())
    for ep in BASE_ENDPOINTS:
        try:
            data=record(directory,args.family,artifact,ep);args.output.write_text(json.dumps(data,indent=2)+"\n",encoding="utf8");break
        except (ValueError,KeyError,OSError) as exc:print(ep,str(exc)[:400],flush=True)
    else:raise ValueError("не получены архивные выплаты Umbrae")
