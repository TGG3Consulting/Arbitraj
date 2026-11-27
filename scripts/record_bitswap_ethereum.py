"""Проверка реестра BitSwap и выплат V5 kind 14. Только чтение и симуляция собственного плательщика."""
import argparse
import json
from pathlib import Path
import requests
from eth_abi import encode, decode
from eth_utils import keccak
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import OWNER, EXEC, RUNTIME, ROOT, STEP, call, calldata, rpc, simulate_block
from record_carbon_ethereum import BLOCK, ENDPOINTS, FUNDING, WETH, STABLES

FIXTURE=ROOT/"tests/data/first_run_dex_2026-11-26/bitswap_ethereum_registry.json"
VIRTUAL="0x"+keccak(text="arb.virtual_quoter")[-20:].hex()
def digest(v):
    return "0x"+keccak(hexstr=v).hex()
def tx(a,name,types=(),values=(),value=0):
    return call(a,calldata(name,types,values),value)
def balance(token,w):
    return tx(token,"balanceOf(address)",["address"],[w])
def read_batch(url, methods):
    out=[]
    for idx in range(0,len(methods),16):
        chunk=methods[idx:idx+16]
        request=[dict(id=i,jsonrpc="2.0",method=k,params=v) for i,(k,v) in enumerate(chunk)]
        reply=requests.post(url,json=request,timeout=40)
        reply.raise_for_status()
        results={r["id"]:r for r in reply.json()}
        out.extend(results[i]["result"] for i in range(len(chunk)))
    return out
def verify(url,tag,j):
    f=j["factory"];markets=j["markets"];mods=list(j["modules"].values())
    roles=[f,*mods,*(m["pool"] for m in markets)]
    actual=list(map(digest,read_batch(url,[("eth_getCode",[a,tag]) for a in roles])))
    if actual!=[j["factory_hash"],*j["module_hashes"],*([j["pool_hash"]]*len(markets))]:
        raise ValueError("different factory, direction modules or pool runtime")
    c=Capture(dict(selector=tag,endpoint=url))
    fields=[]
    slots=[]
    for m in markets:
        pool=m["pool"];x,y=m["tokens"];tier=m["fee"]
        if "0x"+keccak(b"\xff"+bytes.fromhex(f[2:])+keccak(encode(["address","address","uint24"],[x,y,tier]))+bytes.fromhex(j["init"][2:]))[-20:].hex()!=pool:
            raise ValueError("CREATE2 address differs")
        fields.extend([(pool,n+"()",[],[]) for n in ["factory","tokenX","tokenY","fee","pointDelta"]])
        fields.extend([(f,"pool(address,address,uint24)",["address","address","uint24"],[a,b,tier]) for a,b in [(x,y),(y,x)]])
        slots.extend(("eth_getStorageAt",[pool,hex(slot),tag]) for slot in j["module_slots"])
    got=posts(c,fields);targets=read_batch(url,slots)
    for i,m in enumerate(markets):
        data=got[i*7:i*7+7];pool=m["pool"];x,y=m["tokens"]
        if [int.from_bytes(b,"big") for b in data]!=[int(f,16),int(x,16),int(y,16),m["fee"],m["step"],int(pool,16),int(pool,16)]:
            raise ValueError(("role mismatch",pool))
        if [int(v,16) for v in targets[i*5:i*5+5]]!=[int(a,16) for a in mods]:
            raise ValueError(("delegated roles differ",pool))
def capture(url):
    j=json.loads(FIXTURE.read_text());tag={"blockHash":BLOCK,"requireCanonical":True}
    header=rpc(url,"eth_getBlockByHash",[BLOCK,False])
    if int(header["number"],16)!=j["height"] or rpc(url,"eth_chainId",[])!="0x1":
        raise ValueError("canonical anchor")
    verify(url,tag,j)
    end=int(header["timestamp"],16)+240
    word=lambda x:"0x"+format(x,"064x")
    overrides={OWNER:{"balance":hex(30*10**18)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                            "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}},
               VIRTUAL:{"code":"0x"+(ROOT/"contracts/out/VirtualQuoter.bin-runtime").read_text().strip().removeprefix("0x")}}
    cases=[];inactive=[]
    for m in j["markets"]:
        x,y=m["tokens"]
        if WETH not in [x,y] or not ({x,y}&set(STABLES)):continue
        for a,b in [(x,y),(y,x)]:
            for amount in ([10**15,10**16] if a==WETH else [10**6,10**7]):
                if a==WETH:fund=tx(WETH,"deposit()",value=amount)
                else:fund=tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[amount,[WETH,a],OWNER,end],10**18)
                def swap(floor,expiry):
                    return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                          ["address","uint256","address","uint256","uint256",STEP+"[]"],[a,amount,b,floor,expiry,[(14,m["pool"],a,b,10000,b"")]])
                q=tx(VIRTUAL,"quoteIzi(address,bool,uint256)",["address","bool","uint256"],[m["pool"],a==x,amount])
                prep=[fund,tx(a,"approve(address,uint256)",["address","uint256"],[EXEC,amount])]
                quote=simulate_block(url,tag,overrides,prep+[q])
                if not all(r["status"]=="0x1" for r in quote[:2]):raise ValueError("separate funding")
                if quote[-1]["status"]!="0x1":
                    inactive.append(dict(pool=m["pool"],source=a,amount=str(amount),reason=quote[-1].get("error")));continue
                used,out=decode(["uint256","uint256"],bytes.fromhex(quote[-1]["returnData"][2:]))
                if not 0<used<=amount or not out:continue
                before=[balance(t,w) for w in [OWNER,EXEC] for t in [a,b]]
                calls=prep+before+[swap(out+1,end),swap(out,1)]+before+[swap(out,end)]+before
                res=simulate_block(url,tag,overrides,calls)
                if [k for k,r in enumerate(res) if r["status"]!="0x1"]!=[6,7]:
                    raise ValueError(("unexpected swap outcomes",m["pool"],a,amount,[(k,r.get("error")) for k,r in enumerate(res) if r["status"]!="0x1"]))
                frame=lambda i:[int(v["returnData"],16) for v in res[i:i+4]]
                old,mid,new=frame(2),frame(8),frame(13)
                if old!=mid or (old[0]-new[0],new[1]-old[1])!=(used,out) or old[2:]!=new[2:]:
                    raise ValueError("wallet debit/credit, rollback or executor residue")
                cases.append(dict(pool=m["pool"],input=a,output=b,authorized=str(amount),spent=str(used),received=str(out),
                                  swap_gas=int(res[12]["gasUsed"],16),approval_gas=int(res[1]["gasUsed"],16),
                                  before=list(map(str,old)),after=list(map(str,new)),logs=res[12]["logs"],strict=res[6]["error"],expired=res[7]["error"]))
    if not cases:raise ValueError("no payable settlement market proved")
    return dict(selector=tag,state_root=header["stateRoot"],owner=OWNER,executor=EXEC,verified_pools=len(j["markets"]),cases=cases,inactive=inactive)
if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("output",type=Path);args=p.parse_args();errors=[]
    for url in ENDPOINTS:
        try:d=capture(url);args.output.write_text(json.dumps(d,separators=(",",":"))+"\n");break
        except (ValueError,KeyError,RuntimeError,requests.RequestException) as e:errors.append((url,str(e)[:450]))
    else:raise RuntimeError(errors)
