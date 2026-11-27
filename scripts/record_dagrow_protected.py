"""DAGrow. Собственный кошелёк покупает USDT и DAGR, сверяет необработанные суммы и деньги при двух формах заявки."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import BLOCK, ENDPOINTS, FUNDING, WETH, balance, read_batch, tx
from record_dagrow_ethereum import POOL,DAGR,USDT
from record_ethereum_x7 import ROOT_HASH

ADAPTER="0x"+keccak(text="Dagrow SSP archive economic settlement adapter 2026-11-26")[-20:].hex()
PATH=ROOT/"tests/data/first_run_dex_2026-11-26/dagrow_ethereum_directory.json.gz"
def trade(target,kind,a,b,n,limit,expiry):
    if kind in ("module_in","module_out"):
        return tx(target,("swapFrom" if kind=="module_in" else "swapTo")+"(address,address,uint256,uint256,address,uint256)",
                  ["address","address","uint256","uint256","address","uint256"],[a,b,n,limit,OWNER,expiry])
    if kind=="v5":
        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address","uint256","address","uint256","uint256",STEP+"[]"],[a,n,b,limit,expiry,[(29,ADAPTER,a,b,10000,b"")]])
    return tx(target,("swapExactTokensForTokens" if kind=="direct_in" else "swapTokensForExactTokens")+"(uint256,uint256,address[],address,uint256)",
              ["uint256","uint256","address[]","address","uint256"],[n,limit,[a,b],OWNER,expiry])
def word(x): return "0x"+format(x,"064x")
def approval(a,spender,n):return tx(a,"approve(address,uint256)",["address","uint256"],[spender,n])
def capture(endpoint,artifact):
    d=json.loads(gzip.decompress(PATH.read_bytes()));tag=d["selector"]
    chain,head=read_batch(endpoint,[("eth_chainId",[]),("eth_getBlockByHash",[BLOCK,False])])
    if chain!="0x1" or head["hash"]!=BLOCK or head["stateRoot"]!=ROOT_HASH:raise ValueError("иной Ethereum")
    end=int(head["timestamp"],16)+180
    guard=d["roles"]["guard"]["address"]
    if any(v!="0x" for v in read_batch(endpoint,[("eth_getCode",[addr,tag]) for addr in (OWNER,ADAPTER,EXEC)])):raise ValueError("адрес архивного кошелька или модулей занят")
    create={"from":OWNER,"data":"0x"+artifact["creation"]+encode(["address"]*3,[POOL,USDT,DAGR]).hex()}
    concrete,=read_batch(endpoint,[("eth_call",[create,tag])]);runtime=bytearray.fromhex(concrete[2:]);template=bytes.fromhex(artifact["runtime"])
    roles=set()
    for refs in artifact["immutableReferences"].values():
        items={int.from_bytes(runtime[r["start"]:r["start"]+r["length"]],"big") for r in refs}
        if len(items)!=1:raise ValueError("несогласованный immutable")
        roles.update(items)
        for r in refs:
            i,n=r["start"],r["length"];runtime[i:i+n]=template[i:i+n]
    if runtime!=template or roles!={int(a,16) for a in (POOL,DAGR,USDT)}:raise ValueError("шаблон модуля отличается")
    env={OWNER:{"balance":hex(10**19)},ADAPTER:{"code":concrete},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
        "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    run=lambda ops:simulate_block(endpoint,tag,env,ops)
    fund=tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],
            [40*10**6,[WETH,USDT],OWNER,end],10**18)
    buy=trade(POOL,"direct_in",USDT,DAGR,20*10**6,1,end)
    cases=[]
    for a,b,unit in [(USDT,DAGR,10**6),(DAGR,USDT,10**18)]:
        pre=[fund] if a==USDT else [fund,approval(USDT,POOL,20*10**6),buy]
        for qty in (unit,5*unit):
            for mode in ("direct_in","direct_out","module_in","module_out","v5"):
                by_target=mode.endswith("_out");pay=OWNER if mode.startswith("direct") else ADAPTER
                target=POOL if mode.startswith("direct") else EXEC if mode=="v5" else ADAPTER
                def quote(out,n):
                    return tx(POOL,("getAmountsIn" if out else "getAmountsOut")+"(uint256,address[],address)",
                              ["uint256","address[]","address"],[n,[a,b],pay])
                def pair(item):return decode(["uint256[]"],bytes.fromhex(item["returnData"][2:]))[0]
                terms=[tx(guard,"verifySwap(address,bool)",["address","bool"],[pay,a==DAGR]),tx(POOL,"getReserves()"),
                       tx(guard,"guardianTradingHalted()"),tx(guard,"isSellPaused()"),quote(False,qty)]
                z=run(pre+terms);errors=[r.get("error") for r in z if r["status"]!="0x1"]
                if errors:raise ValueError(("нет собственной подготовки или котировки",mode,errors))
                desired=pair(z[-1])[1]; cap=qty
                if by_target:
                    z0=run(pre+[quote(True,desired)]);cap=pair(z0[-1])[0]
                base=pre+[approval(a,target,0),approval(a,target,cap)]
                reads=[balance(t,who) for who in (OWNER,ADAPTER,EXEC) for t in (a,b)]
                reads+=[tx(a,"allowance(address,address)",["address","address"],[ADAPTER,POOL])]
                def probe(limit,expiry):
                    n=desired if by_target else qty
                    op=trade(target,mode,a,b,n,limit,expiry)
                    replies=run(base+reads+[op]+reads);i=len(base)+len(reads)
                    if any(r["status"]!="0x1" for k,r in enumerate(replies) if k!=i):raise ValueError("остатки или оплата недоступны")
                    val=lambda rs:[int(r["returnData"],16) for r in rs]
                    return op,replies[i],val(replies[len(base):i]),val(replies[i+1:]),[int(v["gasUsed"],16) for v in replies[len(pre):len(base)]]
                bound=cap if by_target else desired
                op,r,before,after,apgas=probe(bound,end)
                spent,received=before[0]-after[0],after[1]-before[1]
                if r["status"]!="0x1" or spent!=(cap if by_target else qty) or received<desired or before[2:]!=after[2:] or after[-1]:
                    raise ValueError(("не совпали баланс, лимит, allowance или условие исполнения",mode,qty,r.get("error"),spent,received))
                failures=[]
                for m,expiry in [(cap-1 if by_target else received+1,end),(bound,1)]:
                    _,bad,bs,af,_=probe(m,expiry)
                    if bad["status"]!="0x0" or bs!=af:raise ValueError(("строгий предел или срок не удержан",mode,m))
                    failures.append(bad.get("error"))
                cases.append(dict(mode=mode,payer=pay,target=target,quantity=str(qty),input=a,output=b,limit=str(bound),spent=str(spent),received=str(received),
                                  tier=int(z[-5]["returnData"],16),reserves=list(map(str,decode(["uint112","uint112","uint32"],bytes.fromhex(z[-4]["returnData"][2:]))[:2])),
                                  before=list(map(str,before)),after=list(map(str,after)),gas=int(r["gasUsed"],16),approvals=apgas,call=op,logs=r["logs"],failures=failures))
    return dict(selector=tag,state_root=ROOT_HASH,owner=OWNER,module=ADAPTER,executor=EXEC,roles=d["roles"],cases=cases,
                runtime_hash="0x"+keccak(hexstr=concrete).hex(),module_bytes=len(runtime),immutable_references=artifact["immutableReferences"])
if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("artifact",type=Path);ap.add_argument("output",type=Path);args=ap.parse_args(); errors=[]
    for ep in ENDPOINTS:
        try:
            result=capture(ep,json.loads(args.artifact.read_bytes()));args.output.write_text(json.dumps(result,separators=(",",":")));break
        except (requests.RequestException,ValueError,KeyError,RuntimeError) as e:errors.append((ep,str(e)[:360]))
    else:raise ValueError(errors)
