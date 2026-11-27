"""Concave Gemswap. Фабрика и её клоны типа 1, фактическая выплата официальным Router и V5."""
import argparse
from collections import deque
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_bitswap_ethereum import BLOCK, ENDPOINTS, EXEC, FUNDING, OWNER, ROOT, RUNTIME, STABLES, WETH, balance, read_batch, rpc, simulate_block, tx
from record_v2_chain_pools import swap

FACTORY="0x066a5cb7ddc6d55384e2f6ca13d5dd2cd2685cbd"
IMPL="0xe8e429cb147ca1b984eb4e43b8813e6ddc36db6c"
ROUTER="0x0a3e1c20b5384eb97d2ccff9a96bc91f0c77e7db"
DAI="0x6b175474e89094c44da98b954eedeac495271d0f"
HASHES=["0xb8bbb35b9d6b98551279242e2faa151b9312a1116b42a834703aff345c864c6a",
        "0x50dbd82bb11f975de80daae8ce38325a829fa49c8fd35f6744eacbe868a81043",
        "0x7cdc96c6035f5789974d9189d0bb14804b0bb40e75ac8a4dd2d2450e52fe8708"]
REFERENCE="0x91a795615100ff5d479caf4098a688066138cfaa"
SOURCES=["https://github.com/ConcaveFi/concave-frontend/blob/267bfde0775e9daef88564bfdc0aa58c3e4da244/libs/core/src/constants/addresses.ts",
         "https://github.com/ConcaveFi/concave-core/tree/9bd1ce0c8e17b215197227f838d68a7d87807a67/src/amm"]


def approval(a,to,n):
    return tx(a,"approve(address,uint256)",["address","uint256"],[to,n])


def router(a,b,n,limit,end):
    return tx(ROUTER,"swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
              ["uint256","uint256","address[]","address","uint256"],[n,limit,[a,b],OWNER,end])


def registry(url,tag):
    def views(calls):
        results=read_batch(url,[("eth_call",[c,tag]) for c in calls])
        return [bytes.fromhex(r[2:]) for r in results]
    def addresses(values):
        return ["0x"+r[-20:].hex() for r in values]
    def code(targets):
        return [bytes.fromhex(r[2:]) for r in read_batch(url,[("eth_getCode",[a,tag]) for a in targets])]
    roles={}
    for address,h,code_bytes in zip((FACTORY,IMPL,ROUTER),HASHES,code([FACTORY,IMPL,ROUTER]),strict=True):
        if "0x"+keccak(code_bytes).hex()!=h:
            raise ValueError(("different canonical deployment",address))
        roles[address]={"hash":h,"bytes":len(code_bytes)}
    fields=views([tx(FACTORY,"allPairsLength()"),tx(FACTORY,"typeToImplementation(uint256)",["uint256"],[1]),
                  tx(ROUTER,"factory()"),tx(ROUTER,"implementation()"),tx(ROUTER,"WETH()")])
    count=int.from_bytes(fields[0],"big")
    if addresses(fields[1:])!=[IMPL,FACTORY,IMPL,WETH] or not 0<count<100_000:
        raise ValueError("unknown router roles or registry length")
    pools=addresses(views([tx(FACTORY,"allPairs(uint256)",["uint256"],[i]) for i in range(count)]))
    if len(set(pools))!=count or REFERENCE not in pools:
        raise ValueError("registry identity")
    template=bytes.fromhex("363d3d373d3d3d363d73"+IMPL[2:]+"5af43d82803e903d91602b57fd5bf3")
    init=keccak(bytes.fromhex("3d602d80600a3d3981f3")+template)
    fields=views([tx(pool,func) for pool in pools for func in ("token0()","token1()","getReserves()","swapFee()")])
    deployed=code(pools)
    rows=[]
    for i,pool in enumerate(pools):
        x,y=addresses(fields[i*4:i*4+2])
        checks=views([tx(FACTORY,"getPair(address,address,uint256)",["address","address","uint256"],[a,b,1]) for a,b in [(x,y),(y,x)]]+
                     [tx(FACTORY,"getPair(address,address)",["address","address"],[x,y]),
                      balance(x,pool),balance(y,pool)])
        salt=keccak(bytes.fromhex(x[2:]+y[2:])+(1).to_bytes(32,"big"))
        predicted="0x"+keccak(b"\xff"+bytes.fromhex(FACTORY[2:])+salt+init)[-20:].hex()
        if x>=y or deployed[i]!=template or predicted!=pool or addresses(checks[:3])!=[pool]*3:
            raise ValueError(("not a registered type-1 clone",pool))
        r0,r1,_=decode(["uint112","uint112","uint32"],fields[4*i+2])
        rows.append(dict(pool=pool,tokens=[x,y],reserves=[str(r0),str(r1)],balances=[str(int.from_bytes(v,"big")) for v in checks[-2:]],
                         fee_bps=int.from_bytes(fields[4*i+3],"big")))
    return dict(roles=roles,clone_hash="0x"+keccak(template).hex(),clone_init="0x"+init.hex(),markets=rows)


def funded(markets,target,tested,run,end):
    seeds={WETH:10**15, DAI:10**18, STABLES[0]:10**6, STABLES[1]:10**6}
    paths=[]
    queue=deque((token,[]) for token in seeds)
    while queue:
        a,path=queue.popleft()
        if a==target:
            paths.append(path)
        elif len(path)<3:
            for m in markets:
                if a not in m["tokens"] or m["pool"] in [q[2] for q in path]:continue
                b=next(t for t in m["tokens"] if t!=a)
                if b in [q[0] for q in path]:continue
                queue.append((b,path+[(a,b,m["pool"])]))
    for path in sorted(paths,key=lambda ls:(any(h[2]==tested for h in ls),len(ls))):
        seed=path[0][0] if path else target
        n=seeds[seed]
        fund=tx(WETH,"deposit()",value=n) if seed==WETH else tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                 ["uint256","address[]","address","uint256"],[n,[WETH,seed],OWNER,end],10**17)
        prefix=[fund]
        for a,b,pool in path:
            leg=[approval(a,ROUTER,n),router(a,b,n,1,end)]
            trial=run(prefix+[balance(b,OWNER)]+leg+[balance(b,OWNER)])
            if any(r["status"]!="0x1" for r in trial):break
            n=int(trial[-1]["returnData"],16)-int(trial[-4]["returnData"],16)
            if n<2:break
            prefix.extend(leg)
        else:return prefix,n,path
    return None


def capture(url):
    tag={"blockHash":BLOCK,"requireCanonical":True}
    header=rpc(url,"eth_getBlockByHash",[BLOCK,False]);end=int(header["timestamp"],16)+200
    if header["hash"]!=BLOCK or rpc(url,"eth_chainId",[])!="0x1":raise ValueError("wrong chain or header")
    d=registry(url,tag)
    if any(x!="0x" for x in read_batch(url,[("eth_getCode",[a,tag]) for a in [OWNER,EXEC]])):
        raise ValueError("simulation address already occupied")
    word=lambda v:"0x"+format(v,"064x")
    overrides={OWNER:{"balance":hex(10**18)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
               "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    run=lambda ls:simulate_block(url,tag,overrides,ls)
    fills=[];unavailable=[]
    for m in d["markets"]:
        for k,a in enumerate(m["tokens"]):
            b=m["tokens"][1-k];pair=m["pool"]
            account=funded(d["markets"],a,pair,run,end)
            if not account:
                unavailable.append(dict(pool=pair,source=a,phase="funding"))
                continue
            pre,quantity,path=account
            reserves=run(pre+[tx(pair,"getReserves()")])
            if any(v["status"]!="0x1" for v in reserves):raise ValueError("funded state unavailable")
            rs=decode(["uint112","uint112","uint32"],bytes.fromhex(reserves[-1]["returnData"][2:]))
            capacity=min(quantity,rs[k])
            smaller=max(1,capacity//100)
            larger=min(quantity,max(smaller+1,capacity//10))
            for n in sorted({smaller,larger} - {0}):
                for name,spender in [("router",ROUTER),("v5",EXEC)]:
                    base=pre+[approval(a,spender,n)]
                    stage=lambda minimum,expiry:router(a,b,n,minimum,expiry) if name=="router" else swap(pair,a,b,n,minimum,expiry,m["fee_bps"]*100)
                    bal=[balance(t,w) for w in (OWNER,EXEC) for t in (a,b)]
                    trial=run(base+bal+[stage(1,end)]+bal)
                    i=len(base);result=trial[i+4]
                    if any(v["status"]!="0x1" for v in trial[:i+4]):raise ValueError("preparation or balance reverted")
                    if result["status"]!="0x1":
                        unavailable.append(dict(pool=pair,source=a,n=str(n),wire=name,reason=result.get("error")))
                        continue
                    words=lambda ls:[int(v["returnData"],16) for v in ls]
                    old=words(trial[i:i+4]);new=words(trial[-4:]);out=new[1]-old[1]
                    if old[0]-new[0]!=n or old[2:]!=new[2:] or out<=0:raise ValueError(("wallet debit, payout or residue",pair,name))
                    controls=run(base+bal+[stage(out+1,end),stage(out,1)]+bal+[stage(out,end)]+bal)
                    if [n0 for n0,r in enumerate(controls) if r["status"]!="0x1"]!=[i+4,i+5] or words(controls[i+6:i+10])!=old or words(controls[-4:])!=new:
                        raise ValueError(("payout floor, expiry, balance after rollback",pair,name))
                    fills.append(dict(pool=pair,source=a,target=b,wire=name,funding=path,spent=str(n),received=str(out),
                        before=list(map(str,old)),after=list(map(str,new)),reserves=list(map(str,rs[:2])),gas=int(controls[i+10]["gasUsed"],16),
                        approval_gas=int(controls[i-1]["gasUsed"],16),logs=controls[i+10]["logs"],
                        strict=controls[i+4]["error"],expired=controls[i+5]["error"]))
    if not any(v["pool"]==REFERENCE for v in fills):raise ValueError("no payable pCNV/DAI leg")
    d.update(selector=tag,state_root=header["stateRoot"],sources=SOURCES,owner=OWNER,cases=fills,unavailable=unavailable)
    return d


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path);args=parser.parse_args();errors=[]
    for endpoint in ENDPOINTS:
        try:
            result=capture(endpoint)
            args.output.write_text(json.dumps(result,separators=(",",":"))+"\n",encoding="utf8")
            break
        except (requests.RequestException,ValueError,KeyError,RuntimeError) as exc:
            errors.append((endpoint,str(exc)[:500]))
    else:raise RuntimeError(errors)
