"""WAGMI и Lif3 Ethereum. Регистрация CREATE2 и расчёт собственного кошелька через штатные роутеры и V5."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_bitswap_ethereum import ENDPOINTS, EXEC, FUNDING, OWNER, ROOT, RUNTIME, WETH, balance, read_batch, rpc, simulate_block, tx
from record_base_slipstream_executor import STEP
from record_concave_ethereum import approval
from record_fraxswap_swaps import bound_runtime
from record_registered_v3_module import ADAPTER
from record_ethereum_x7 import ROOT_HASH

DATA=ROOT/"tests/data/first_run_dex_2026-11-26"
STABLE="0xdac17f958d2ee523a2206206994597c13d831ec7"
CREATED="0x"+keccak(text="PoolCreated(address,address,uint24,int24,address)").hex()
SWAP="0x"+keccak(text="Swap(address,address,int256,int256,uint160,uint128,int24)").hex()
QUOTE="(address,address,uint256,uint24,uint160)"
ORDER="(address,address,uint24,address,uint256,uint256,uint256,uint160)"
ORDER02="(address,address,uint24,address,uint256,uint256,uint160)"

def registry(url,d,tag):
    markets=d["markets"];roles=d["roles"];f=roles["factory"]["address"]
    raw=read_batch(url,[("eth_getCode",[r["address"],tag]) for r in roles.values()])
    if any("0x"+keccak(hexstr=v).hex()!=r["runtime"] for r,v in zip(roles.values(),raw,strict=True)):
        raise ValueError("код одной из официальных ролей изменился")
    def views(calls):
        values=[]
        for start in range(0,len(calls),48):
            group=calls[start:start+48]
            packed=[(c["to"],False,bytes.fromhex(c["data"][2:])) for c in group]
            read=tx("0xca11bde05977b3631167028862be2a173976ca11","aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[packed])
            read["gas"]=hex(12_000_000)
            raw,=read_batch(url,[("eth_call",[read,tag])])
            results,=decode(["(bool,bytes)[]"],bytes.fromhex(raw[2:]))
            if len(results)!=len(group) or any(not ok for ok,_ in results):raise ValueError("неполные данные рынка")
            values.extend(v for _,v in results)
        return values
    def address(data):
        return decode(["address"],data)[0]
    for role in (role for role in ("router","router02","quoter") if role in roles):
        target=roles[role]["address"]
        if [address(b) for b in views([tx(target,"factory()"),tx(target,"WETH9()")])]!=[f,WETH]:
            raise ValueError(("назначение роли",role))
    deployer=d.get("deployer",f)
    nonce=int(rpc(url,"eth_getTransactionCount",[deployer,tag]),16)
    if nonce!=len(markets)+1 or len({m["pool"] for m in markets})!=len(markets):
        raise ValueError("реестр не покрывает все создания фабрики")
    receipts=read_batch(url,[("eth_getTransactionReceipt",[m["tx"]]) for m in markets])
    missing = [(i,m) for i,(m,r) in enumerate(zip(markets,receipts,strict=True)) if r is None]
    if missing:
        material = {}
        session = requests.Session()
        def history(node, method, params):
            if method not in ("eth_getBlockByHash","eth_chainId","eth_getTransactionReceipt","eth_getLogs"):
                raise ValueError("unrecognized historical read")
            res=session.post(node,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=25)
            res.raise_for_status()
            return res.json()["result"]
        for node in dict.fromkeys([url,*ENDPOINTS,"https://eth.drpc.org"]):
            if not missing: break
            try:
                anchor=history(node,"eth_getBlockByHash",[tag["blockHash"],False])
                if not anchor or anchor["stateRoot"]!=d.get("state_root",ROOT_HASH) or history(node,"eth_chainId",[])!="0x1":
                    continue
                for i,m in missing:
                    if i in material: continue
                    r=history(node,"eth_getTransactionReceipt",[m["tx"]])
                    if isinstance(r,dict) and r.get("transactionHash","").lower()==m["tx"].lower() and r.get("blockHash")==m["hash"]:
                        material[i]=r;continue
                    records=history(node,"eth_getLogs",[{"blockHash":m["hash"],"address":f,"topics":[CREATED]}])
                    records=[row for row in records if row["transactionHash"].lower()==m["tx"].lower()
                             and row["blockHash"]==m["hash"] and not row.get("removed") and int(row["logIndex"],16)==m["index"]]
                    if len(records)==1:
                        material[i]={"blockHash":m["hash"],"logs":records}
            except (KeyError,ValueError,RuntimeError,requests.RequestException):
                pass
            missing=[(i,m) for i,m in missing if i not in material]
        if missing: raise ValueError(("missing canonical PoolCreated", [m["tx"] for _,m in missing]))
        for i,r in material.items():receipts[i]=r
    calls=[]
    for m,receipt in zip(markets,receipts,strict=True):
        a,b=m["tokens"];fee=m["fee"]
        logs=[l for l in receipt["logs"] if int(l["logIndex"],16)==m["index"]]
        if len(logs)!=1 or receipt["blockHash"]!=m["hash"] or ("status" in receipt and receipt["status"]!="0x1"):raise ValueError("receipt регистрации")
        event=logs[0];spacing,pool=decode(["int24","address"],bytes.fromhex(event["data"][2:]))
        predicted="0x"+keccak(b"\xff"+bytes.fromhex(deployer[2:])+keccak(encode(["address","address","uint24"],[a,b,fee]))+bytes.fromhex(d["init"][2:]))[-20:].hex()
        if (pool,spacing)!=(m["pool"],m["spacing"]) or predicted!=pool or event["address"].lower()!=f or event["topics"]!=[CREATED,*["0x"+format(int(v,16),"064x") for v in (a,b)],"0x"+format(fee,"064x")]:
            raise ValueError(("CREATE2 и PoolCreated",pool))
        calls.extend([tx(pool,k+"()") for k in ("token0","token1","factory","fee","tickSpacing","liquidity","slot0")]+
                     [tx(f,"getPool(address,address,uint24)",["address","address","uint24"],[x,y,fee]) for x,y in [(a,b),(b,a)]]+
                     [balance(a,pool),balance(b,pool)])
    raw=views(calls)
    for i,m in enumerate(markets):
        v=raw[11*i:11*i+11];a,b=m["tokens"];p=m["pool"]
        if [address(v[j]) for j in (0,1,2,7,8)]!=[a,b,f,p,p] or [int.from_bytes(v[j],"big") for j in (3,4)]!=[m["fee"],m["spacing"]]:
            raise ValueError(("поля рынка",p))
        m.update(liquidity=str(int.from_bytes(v[5],"big")),slot0="0x"+v[6].hex(),balances=[str(int.from_bytes(w,"big")) for w in v[9:]])
    checks=[]
    for m in markets:
        tick=decode(["uint160","int24"],bytes.fromhex(m["slot0"][2:])[:64])[1]
        word=(tick//m["spacing"])//256
        for index in range(max(-32768,word-1),min(32767,word+1)+1):
            checks.append((m,index))
    reads=views([call for m,index in checks for call in (tx(m["pool"],"tickBitmap(int16)",["int16"],[index]),
                  tx(roles["lens"]["address"],"getPopulatedTicksInWord(address,int16)",["address","int16"],[m["pool"],index]))])
    ticks=[]
    for idx,(m,word) in enumerate(checks):
        bitmap=int.from_bytes(reads[2*idx],"big")
        rows,=decode(["(int24,int128,uint128)[]"],reads[2*idx+1])
        indexes={(word*256+bit)*m["spacing"] for bit in range(256) if bitmap&(1<<bit)}
        if indexes!={t for t,_,_ in rows} or len(indexes)!=len(rows):raise ValueError(("TickLens bitmap",m["pool"],word))
        ticks.extend((m,t,net,gross) for t,net,gross in rows)
        m.setdefault("tick_words",[]).append(dict(word=word,bitmap=hex(bitmap),rows=[[t,str(net),str(gross)] for t,net,gross in rows]))
    raw=views([tx(m["pool"],"ticks(int24)",["int24"],[t]) for m,t,_,_ in ticks])
    for (m,t,net,gross),v in zip(ticks,raw,strict=True):
        if decode(["uint128","int128"],v[:64])!=(gross,net):raise ValueError(("TickLens gross/net",m["pool"],t))


def capture(url,venue,artifact=None):
    expanded=venue in ("blueprint","vvs")
    if expanded:
        d=json.loads(gzip.decompress((DATA/f"{venue}_ethereum_directory.json.gz").read_bytes()))
    else:
        d=json.loads((DATA/f"{venue}_ethereum_registry.json").read_bytes())
    tag=d["selector"];head=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False]);end=int(head["timestamp"],16)+300
    if int(head["number"],16)!=d["height"] or rpc(url,"eth_chainId",[])!="0x1":raise ValueError("неверный блок Ethereum")
    if head["stateRoot"]!=d.get("state_root",ROOT_HASH):raise ValueError("different archive root")
    registry(url,d,tag)
    if read_batch(url,[("eth_getCode",[a,tag]) for a in (OWNER,EXEC)])!=["0x","0x"]:raise ValueError("тестовый адрес занят")
    word=lambda x:"0x"+format(x,"064x")
    changes={OWNER:{"balance":hex(10**18)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
            "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    if venue=="vvs":
        if rpc(url,"eth_getCode",[ADAPTER,tag])!="0x" or not artifact:raise ValueError("VVS isolated module or build")
        code=bound_runtime(artifact,d["factory"],url,tag)
        changes[ADAPTER]={"code":"0x"+code}
        d["module_hash"]="0x"+keccak(hexstr=code).hex()
    run=lambda calls:simulate_block(url,tag,changes,calls)
    roles={k:v["address"] for k,v in d["roles"].items()};output=[];missing=[]
    usdc="0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
    wbtc="0x2260fac5e5542a773aa44fbcfedf7c193bc2c599"
    budgets={WETH:([10**14,10**16] if expanded else [10**12,10**14]),STABLE:([100_000,1_000_000] if expanded else [1000,100000]),usdc:[100_000,1_000_000],wbtc:[100,1000]}
    swap_signature=SWAP if venue!="vvs" else "0x"+keccak(text="Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)").hex()
    modes=("router","module","v5") if venue=="vvs" else (("router","router02","v5") if "router02" in roles else ("router","v5"))
    for m in d["markets"]:
        pool=m["pool"];fee=m["fee"]; cash=next((a for a in budgets if a in m["tokens"]),None)
        if expanded and (cash is None or not all(int(n)>0 for n in m["balances"])):
            missing.append(dict(pool=pool,reason="no measured funding source" if cash is None else "one reserve empty on captured block"));continue
        if cash is None:raise ValueError("нужен собственный источник входа")
        other=next(t for t in m["tokens"] if t!=cash)
        for seed in budgets[cash]:
            fund=tx(WETH,"deposit()",value=seed) if cash==WETH else tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[seed,[WETH,cash],OWNER,end],10**16)
            for wire in modes:
                spender=EXEC if wire=="v5" else (ADAPTER if wire=="module" else roles[wire]);prefix=[fund];amount=seed
                for a,b in [(cash,other),(other,cash)]:
                    permit=approval(a,spender,amount)
                    quote=tx(roles["quoter"],"quoteExactInputSingle("+QUOTE+")",[QUOTE],[(a,b,amount,fee,0)])
                    def order(minimum,deadline):
                        if wire=="module":
                            return tx(ADAPTER,"execute(address,address,address,uint256,uint256,uint256)",["address","address","address","uint256","uint256","uint256"],[pool,a,b,amount,minimum,deadline])
                        if wire=="v5":
                            step=(17,ADAPTER,a,b,10000,encode(["address"],[pool])) if venue=="vvs" else (1,pool,a,b,10000,b"")
                            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address","uint256","address","uint256","uint256",STEP+"[]"],[a,amount,b,minimum,deadline,[step]])
                        if wire=="router":
                            return tx(spender,"exactInputSingle("+ORDER+")",[ORDER],[(a,b,fee,OWNER,deadline,amount,minimum,0)])
                        inner=tx(spender,"exactInputSingle("+ORDER02+")",[ORDER02],[(a,b,fee,OWNER,amount,minimum,0)])
                        return tx(spender,"multicall(uint256,bytes[])",["uint256","bytes[]"],[deadline,[bytes.fromhex(inner["data"][2:])]])
                    checks=[balance(t,w) for w in ((OWNER,EXEC,ADAPTER) if venue=="vvs" else (OWNER,EXEC)) for t in (a,b)]
                    width=len(checks)
                    base=prefix+[permit,*checks,quote];i=len(base)
                    trial=run(base+[order(1,end),*checks])
                    if any(v["status"]!="0x1" for v in trial[:i-1]):raise ValueError("финансирование или баланс")
                    if trial[i-1]["status"]!="0x1" or trial[i]["status"]!="0x1":
                        missing.append(dict(pool=pool,wire=wire,source=a,amount=str(amount),quote=trial[i-1].get("error"),swap=trial[i].get("error")));break
                    if any(v["status"]!="0x1" for v in trial[-width:]):raise ValueError("баланс после выплаты")
                    words=lambda rs:[int(v["returnData"],16) for v in rs]
                    before=words(trial[i-width-1:i-1]);after=words(trial[-width:]);spent=before[0]-after[0];out=after[1]-before[1]
                    q=decode(["uint256","uint160","uint32","uint256"],bytes.fromhex(trial[i-1]["returnData"][2:]))
                    if spent!=amount or out!=q[0] or out<=0 or before[2:]!=after[2:]:
                        missing.append(dict(pool=pool,wire=wire,source=a,amount=str(amount),spent=str(spent),net=str(out),quoted=str(q[0]),before=list(map(str,before)),after=list(map(str,after))));break
                    exact=order(out,end);rr=run(base+[order(out+1,end),order(out,1),*checks,exact,*checks])
                    if [k for k,v in enumerate(rr) if v["status"]!="0x1"]!=[i,i+1] or words(rr[i+2:i+2+width])!=before or words(rr[-width:])!=after:
                        raise ValueError(("min, срок, исходный и конечный балансы",wire,pool))
                    result=rr[i+2+width]
                    settlements=[v for v in result["logs"] if v["address"].lower()==pool and v["topics"][0].lower()==swap_signature]
                    if len(settlements)!=1:raise ValueError(("фактический Swap",wire,pool))
                    realized=decode(["int256","int256","uint160","uint128","int24"],bytes.fromhex(settlements[0]["data"][2:]))
                    side=m["tokens"].index(a)
                    if realized[side]!=spent or realized[1-side]!=-out or realized[2]!=q[1]:
                        raise ValueError(("реальные pool delta и sqrtPrice после сделки",wire,pool))
                    output.append(dict(pool=pool,wire=wire,source=a,target=b,spent=str(spent),received=str(out),quote=[str(n) for n in q],gas=int(result["gasUsed"],16),
                                       signed=exact,minimum_plus_one=rr[i].get("error"),expired=rr[i+1].get("error"),approval_gas=int(rr[len(prefix)]["gasUsed"],16),before=list(map(str,before)),after=list(map(str,after)),logs=result["logs"]))
                    prefix+=[permit,exact];amount=out
    if expanded and not all(any(r["wire"]==wire and r["target"] in budgets for r in output) for wire in modes):
        raise ValueError("missing confirmed return leg for a selected wire")
    d.update(cases=output,unavailable=missing)
    return d

if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__);cli.add_argument("output",type=Path);cli.add_argument("--venue",choices=["wagmi","lif3","blueprint","vvs"],default="wagmi");cli.add_argument("--module",type=Path);args=cli.parse_args()
    errors=[]
    for url in ENDPOINTS:
        try:
            args.output.write_text(json.dumps(capture(url,args.venue,json.loads(args.module.read_bytes()) if args.module else None),separators=(",",":")),encoding="utf8");break
        except (ValueError,KeyError,RuntimeError,requests.RequestException) as err:
            errors.append((url,str(err)[:650]))
    else:raise RuntimeError(errors)
