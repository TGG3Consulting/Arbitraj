"""WAGMI Ethereum. Регистрация CREATE2 и выплата из собственного входа через SwapRouter, Router02 и V5."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_bitswap_ethereum import ENDPOINTS, EXEC, FUNDING, OWNER, ROOT, RUNTIME, WETH, balance, read_batch, rpc, simulate_block, tx
from record_base_slipstream_executor import STEP
from record_concave_ethereum import approval

REGISTRY=ROOT/"tests/data/first_run_dex_2026-11-26/wagmi_ethereum_registry.json"
STABLE="0xdac17f958d2ee523a2206206994597c13d831ec7"
CREATED="0x"+keccak(text="PoolCreated(address,address,uint24,int24,address)").hex()
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
            raw=rpc(url,"eth_call",[read,tag])
            results,=decode(["(bool,bytes)[]"],bytes.fromhex(raw[2:]))
            if len(results)!=len(group) or any(not ok for ok,_ in results):raise ValueError("неполные данные рынка")
            values.extend(v for _,v in results)
        return values
    def address(data):
        return decode(["address"],data)[0]
    for role in ("router","router02","quoter"):
        target=roles[role]["address"]
        if [address(b) for b in views([tx(target,"factory()"),tx(target,"WETH9()")])]!=[f,WETH]:
            raise ValueError(("назначение роли",role))
    nonce=int(rpc(url,"eth_getTransactionCount",[f,tag]),16)
    if nonce!=len(markets)+1 or len({m["pool"] for m in markets})!=len(markets):
        raise ValueError("реестр не покрывает все создания фабрики")
    receipts=read_batch(url,[("eth_getTransactionReceipt",[m["tx"]]) for m in markets])
    calls=[]
    for m,receipt in zip(markets,receipts,strict=True):
        a,b=m["tokens"];fee=m["fee"]
        logs=[l for l in receipt["logs"] if int(l["logIndex"],16)==m["index"]]
        if len(logs)!=1 or receipt["blockHash"]!=m["hash"] or receipt["status"]!="0x1":raise ValueError("receipt регистрации")
        event=logs[0];spacing,pool=decode(["int24","address"],bytes.fromhex(event["data"][2:]))
        predicted="0x"+keccak(b"\xff"+bytes.fromhex(f[2:])+keccak(encode(["address","address","uint24"],[a,b,fee]))+bytes.fromhex(d["init"][2:]))[-20:].hex()
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

def capture(url):
    d=json.loads(REGISTRY.read_bytes());tag=d["selector"];head=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False]);end=int(head["timestamp"],16)+300
    if int(head["number"],16)!=d["height"] or rpc(url,"eth_chainId",[])!="0x1":raise ValueError("неверный блок Ethereum")
    registry(url,d,tag)
    if read_batch(url,[("eth_getCode",[a,tag]) for a in (OWNER,EXEC)])!=["0x","0x"]:raise ValueError("тестовый адрес занят")
    word=lambda x:"0x"+format(x,"064x")
    changes={OWNER:{"balance":hex(10**18)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
            "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    run=lambda calls:simulate_block(url,tag,changes,calls)
    roles={k:v["address"] for k,v in d["roles"].items()};output=[];missing=[]
    for m in d["markets"]:
        pool=m["pool"];fee=m["fee"];cash=WETH if WETH in m["tokens"] else STABLE
        if cash not in m["tokens"]:raise ValueError("нужен собственный источник входа")
        other=next(t for t in m["tokens"] if t!=cash)
        for seed in ([10**12,10**14] if cash==WETH else [1000,100000]):
            fund=tx(WETH,"deposit()",value=seed) if cash==WETH else tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[seed,[WETH,cash],OWNER,end],10**16)
            for wire in ("router","router02","v5"):
                spender=EXEC if wire=="v5" else roles[wire];prefix=[fund];amount=seed
                for a,b in [(cash,other),(other,cash)]:
                    permit=approval(a,spender,amount)
                    quote=tx(roles["quoter"],"quoteExactInputSingle("+QUOTE+")",[QUOTE],[(a,b,amount,fee,0)])
                    def order(minimum,deadline):
                        if wire=="v5":
                            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address","uint256","address","uint256","uint256",STEP+"[]"],[a,amount,b,minimum,deadline,[(1,pool,a,b,10000,b"")]])
                        if wire=="router":
                            return tx(spender,"exactInputSingle("+ORDER+")",[ORDER],[(a,b,fee,OWNER,deadline,amount,minimum,0)])
                        inner=tx(spender,"exactInputSingle("+ORDER02+")",[ORDER02],[(a,b,fee,OWNER,amount,minimum,0)])
                        return tx(spender,"multicall(uint256,bytes[])",["uint256","bytes[]"],[deadline,[bytes.fromhex(inner["data"][2:])]])
                    checks=[balance(t,w) for w in (OWNER,EXEC) for t in (a,b)]
                    base=prefix+[permit,*checks,quote];i=len(base)
                    trial=run(base+[order(1,end),*checks])
                    if any(v["status"]!="0x1" for v in trial[:i-1]):raise ValueError("финансирование или баланс")
                    if trial[i-1]["status"]!="0x1" or trial[i]["status"]!="0x1":
                        missing.append(dict(pool=pool,wire=wire,source=a,amount=str(amount),quote=trial[i-1].get("error"),swap=trial[i].get("error")));break
                    if any(v["status"]!="0x1" for v in trial[-4:]):raise ValueError("баланс после выплаты")
                    words=lambda rs:[int(v["returnData"],16) for v in rs]
                    before=words(trial[i-5:i-1]);after=words(trial[-4:]);spent=before[0]-after[0];out=after[1]-before[1]
                    q=decode(["uint256","uint160","uint32","uint256"],bytes.fromhex(trial[i-1]["returnData"][2:]))
                    if spent!=amount or out!=q[0] or out<=0 or before[2:]!=after[2:]:
                        missing.append(dict(pool=pool,wire=wire,source=a,amount=str(amount),spent=str(spent),net=str(out),quoted=str(q[0]),before=list(map(str,before)),after=list(map(str,after))));break
                    exact=order(out,end);rr=run(base+[order(out+1,end),order(out,1),*checks,exact,*checks])
                    if [k for k,v in enumerate(rr) if v["status"]!="0x1"]!=[i,i+1] or words(rr[i+2:i+6])!=before or words(rr[-4:])!=after:
                        raise ValueError(("min, срок, исходный и конечный балансы",wire,pool))
                    result=rr[i+6]
                    output.append(dict(pool=pool,wire=wire,source=a,target=b,spent=str(spent),received=str(out),quote=[str(n) for n in q],gas=int(result["gasUsed"],16),
                                       approval_gas=int(rr[len(prefix)]["gasUsed"],16),before=list(map(str,before)),after=list(map(str,after)),logs=result["logs"]))
                    prefix+=[permit,exact];amount=out
    d.update(cases=output,unavailable=missing)
    return d

if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__);cli.add_argument("output",type=Path);args=cli.parse_args()
    errors=[]
    for url in ENDPOINTS:
        try:
            args.output.write_text(json.dumps(capture(url),separators=(",",":")),encoding="utf8");break
        except (ValueError,KeyError,RuntimeError,requests.RequestException) as err:
            errors.append((url,str(err)[:650]))
    else:raise RuntimeError(errors)
