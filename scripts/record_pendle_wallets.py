"""Ethereum Pendle. Покупка PT собственными USDC и погашение позиции через обмен обратно в те же USDC до expiry."""
import argparse
import gzip
import json
from pathlib import Path
import requests
from eth_abi import encode,decode
from eth_utils import keccak
from record_base_slipstream_executor import OWNER,ROOT,EXEC,RUNTIME,STEP,simulate_block
from record_bitswap_ethereum import BLOCK,ENDPOINTS,FUNDING,WETH,tx,balance,token_flow,read_batch
from record_ethereum_x7 import ROOT_HASH
from record_pendle_registry import AT,FACTORIES,ROUTER,ZERO
USDC="0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
TOKEN="(address,uint256,address,address,(uint8,address,bytes,bool))"
ORDER="(uint256,uint256,uint256,uint8,address,address,address,address,uint256,uint256,uint256,bytes)"
FILL="("+ORDER+",bytes,uint256)"
LIMIT="(address,uint256,"+FILL+"[],"+FILL+"[],bytes)"
BUY="swapExactTokenForPtSimple(address,address,uint256,"+TOKEN+")"
SELL="swapExactPtForToken(address,address,uint256,"+TOKEN+","+LIMIT+")"
CORE_SLOT=0xf168c5b0cb4aca9a68f931815c18a144c61ad01d6dd7ca15bd6741672a0ab800
BUY_SY="swapExactSyForPtSimple(address,address,uint256,uint256)"
SELL_SY="swapExactPtForSy(address,address,uint256,uint256,"+LIMIT+")"
def payment(market,quantity,minimum,buy):
    token=(USDC,quantity if buy else minimum,USDC,ZERO,(0,ZERO,b"",False))
    if buy:return tx(ROUTER,BUY,["address","address","uint256",TOKEN],[OWNER,market,minimum,token])
    return tx(ROUTER,SELL,["address","address","uint256",TOKEN,LIMIT],[OWNER,market,quantity,token,(ZERO,0,[],[],b"")])
MODULE="0x"+keccak(text="Arbitraj isolated Ethereum PendleMarketAdapter")[12:].hex()
def route(mode,m,a,b,qty,minimum,expiry,sy):
    if mode=="module":return tx(MODULE,"execute(address,address,address,uint256,uint256,uint256)",["address","address","address","uint256","uint256","uint256"],[m,a,b,qty,minimum,expiry])
    if mode=="v5":
        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address","uint256","address","uint256","uint256",STEP+"[]"],[a,qty,b,minimum,expiry,[(17,MODULE,a,b,10000,encode(["address"],[m]))]])
    if a==sy:return tx(ROUTER,BUY_SY,["address","address","uint256","uint256"],[OWNER,m,qty,minimum])
    if b==sy:return tx(ROUTER,SELL_SY,["address","address","uint256","uint256",LIMIT],[OWNER,m,qty,minimum,(ZERO,0,[],[],b"")])
    return payment(m,qty,minimum,a==USDC)
def capture(url,mode,artifact=None):
    f=ROOT/"tests/data/first_run_dex_2026-11-26/pendle_ethereum_registry.json.gz"
    directory=json.loads(gzip.decompress(f.read_bytes())); sy=directory["standardized_yield"]
    chain,head,user=read_batch(url,[("eth_chainId",[]),("eth_getBlockByHash",[BLOCK,False]),("eth_getCode",[OWNER,AT])])
    if chain!="0x1" or head["stateRoot"]!=ROOT_HASH or user!="0x" or directory["selector"]!=AT:raise ValueError("block or simulation owner")
    env={OWNER:{"balance":hex(10**19)}};runtime=None
    if artifact:
        if any(c!="0x" for c in read_batch(url,[("eth_getCode",[a,AT]) for a in (MODULE,EXEC)])):raise ValueError("адрес модуля/исполнителя занят")
        args=encode(["address","address[5]"],[ROUTER,list(FACTORIES.values())])
        concrete,=read_batch(url,[("eth_call",[{"from":OWNER,"data":"0x"+artifact["creation"]+args.hex()},AT])])
        actual=bytearray.fromhex(concrete[2:]);template=bytes.fromhex(artifact["runtime"]);roles=set()
        for group in artifact["immutableReferences"].values():
            group_roles=set()
            for r in group:
                i,n=r["start"],r["length"];group_roles.add(int.from_bytes(actual[i:i+n],"big"));actual[i:i+n]=template[i:i+n]
            if len(group_roles)!=1:raise ValueError("immutable differs by reference")
            roles.update(group_roles)
        if actual!=template or roles!={int(a,16) for a in [ROUTER,*FACTORIES.values()]}:raise ValueError("wrong module or role")
        runtime="0x"+keccak(hexstr=concrete).hex()
        word=lambda n:"0x"+format(n,"064x")
        env.update({MODULE:{"code":concrete},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),"stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}})
    run=lambda calls:simulate_block(url,AT,env,calls)
    def binding(sig):
        selector=keccak(text=sig)[:4];slot=keccak(encode(["bytes4","uint256"],[selector,CORE_SLOT+2]))
        prop=read_batch(url,[("eth_getStorageAt",[ROUTER,"0x"+slot.hex(),AT])])[0];a="0x"+prop[-40:]
        code=read_batch(url,[("eth_getCode",[a,AT])])[0];return dict(facet=a,bytes=(len(code)-2)//2,runtime_hash="0x"+keccak(hexstr=code).hex())
    bindings={s:binding(s) for s in [BUY,SELL,BUY_SY,SELL_SY]};result=[]
    for r in directory["registrations"]:
        if not r.get("series") or not r.get("router_state") or min(map(int,r["physical"]))<=0 or min(map(int,r["router_state"][:3]))<=0:continue
        syaddr,pt,_=r["tokens"];allowed=sy[syaddr]
        if USDC not in (allowed["in"] or []) or USDC not in (allowed["out"] or []):continue
        m=r["market"]
        for size in [10**6,10*10**6]:
            fund=tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[size+1,[WETH,USDC],OWNER,directory["timestamp"]+300],10**18)
            target={"router":ROUTER,"module":MODULE,"v5":EXEC}[mode]
            approve=lambda token,n:tx(token,"approve(address,uint256)",["address","uint256"],[target,n])
            case={"market":m,"pt":pt,"sy":syaddr,"size":str(size),"expiry":r["expiry"]};result.append(case)
            prefix=[fund]
            if artifact:prefix.append(tx(USDC,"transfer(address,uint256)",["address","uint256"],[MODULE,1]))
            def measure(leg,source,token_in,token_out,amount):
                base=source+[approve(token_in,amount)]
                reads=[balance(token_in,OWNER),balance(token_out,OWNER),balance(r["tokens"][2],OWNER)]
                if syaddr not in (token_in,token_out):reads.append(balance(syaddr,OWNER))
                if artifact:
                    reads.extend([balance(t,h) for h in (MODULE,EXEC) for t in (token_in,token_out,syaddr,r["tokens"][2])])
                    reads.append(tx(token_in,"allowance(address,address)",["address","address"],[MODULE,ROUTER]))
                def probe(minimum,expiry=directory["timestamp"]+240):
                    inventory=[balance(t,ROUTER) for t in (pt,syaddr)]
                    op=route(mode,m,token_in,token_out,amount,minimum,expiry,syaddr);op["gas"]=hex(6_000_000);rs=run(base+inventory+reads+[op]+reads+inventory);idx=len(base)+2+len(reads)
                    if any(x["status"]!="0x1" for j,x in enumerate(rs) if j!=idx):raise ValueError("funds, approvals or reads failed")
                    vals=lambda a:[int(x["returnData"],16) for x in a]
                    if minimum==1 and expiry>1:
                        case.setdefault("router_inventory",{})[leg]=[vals(rs[len(base):len(base)+2]),vals(rs[-2:])]
                    return op,rs[idx],vals(rs[len(base)+2:idx]),vals(rs[idx+1:-2]),int(rs[len(base)-1]["gasUsed"],16)
                _,q,_,_,_=probe(1)
                if q["status"]!="0x1":case[leg]={"stage":"unavailable","error":q.get("error")};return None
                received=token_flow(q["logs"],token_out,OWNER)
                if received<=0:case[leg]={"stage":"no receiver credit"};return None
                op,res,before,after,approval=probe(received)
                if res["status"]!="0x1" or before[0]-after[0]!=amount or after[1]-before[1]!=received or before[2:]!=after[2:] or token_flow(res["logs"],token_in,OWNER)!=-amount:raise ValueError("wallet debit, credit or extra derivative")
                _,strict,bs,af,_=probe(received+1)
                if strict["status"]!="0x0" or bs!=af:raise ValueError("terminal minimum not enforced")
                expiry_error=None
                if artifact:
                    _,expired,be,af,_=probe(received,1)
                    if expired["status"]!="0x0" or be!=af or after[-1]!=0:raise ValueError("deadline/residue/allowance")
                    expiry_error=expired.get("error")
                case[leg]={"stage":"accepted","expiry_error":expiry_error,"input":token_in,"output":token_out,"paid":str(amount),"net":str(received),"before":list(map(str,before)),"after":list(map(str,after)),
                           "gas":int(res["gasUsed"],16),"approval_gas":approval,"returned":res["returnData"],"logs":res["logs"],"transaction":op,"strict":strict.get("error")}
                return base+[op]
            paid=measure("buy",prefix,USDC,pt,size)
            if paid:
                amount=int(case["buy"]["net"])
                if artifact and amount>1:
                    paid=paid+[tx(pt,"transfer(address,uint256)",["address","uint256"],[MODULE,1])];amount-=1
                measure("sell",paid,pt,USDC,amount)
                paid_sy=measure("sell_to_sy",paid,pt,syaddr,amount)
                if paid_sy:
                    measure("buy_with_sy",paid_sy,syaddr,pt,int(case["sell_to_sy"]["net"]))
    return {"mode":mode,"runtime":runtime,"module":MODULE if artifact else None,"executor":EXEC if artifact else None,"selector":AT,"state_root":ROOT_HASH,"payer":OWNER,"router":ROUTER,"bindings":bindings,"terminal":USDC,"cases":result}
if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path);p.add_argument("--module",type=Path);opts=p.parse_args();failures=[]
    for url in ENDPOINTS:
        try:
            output=capture(url,"router") if not opts.module else {mode:capture(url,mode,json.loads(opts.module.read_bytes())) for mode in ("module","v5")}
            opts.output.write_text(json.dumps(output,separators=(",",":")),encoding="utf8");break
        except (ValueError,KeyError,RuntimeError,requests.RequestException) as e:failures.append(str(e)[:240])
    else:raise RuntimeError(failures)
