"""Ethereum WhiteSwap: funded EOA vs ArbExecutor V5 at a single canonical state.
No broadcast, signing keys, synthetic token balance or token storage overrides.
"""
import json
import sys
import time
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_base_slipstream_executor import EXEC, ROOT, RUNTIME, STEP, calldata, call, simulate_block

FIXTURE = ROOT / "tests/data/first_run_dex_2026-11-26/whiteswap_wallets.json"
ETH = ROOT / "tests/data/first_run_dex_2026-11-26/ethereum.json"
SLOT = hex(int.from_bytes(keccak(text="eip1967.proxy.implementation"),"big")-1)
READS = {"eth_chainId","eth_getBlockByNumber","eth_getCode","eth_getStorageAt","eth_simulateV1"}


def request(url, method, params):
    if method not in READS:
        raise ValueError("not an archive read")
    r = requests.post(url,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=(4,45))
    r.raise_for_status()
    msg = r.json()
    if msg.get("id")!=1 or "error" in msg or msg.get("result") is None:
        raise RuntimeError(str(msg.get("error","missing result"))[:150])
    return msg["result"]


def amount_out(amount, reserve_in, reserve_out, discount):
    net=amount*(9970+discount)
    return net*reserve_out//(10000*reserve_in+net)


def run(out):
    source=json.loads(FIXTURE.read_bytes())
    tag=source["selector"]
    weth="0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
    router,pool=source["router"],source["pool"]
    tokens=source["tokens"]
    nodes=[]
    for url in ("https://eth.drpc.org","https://ethereum-rpc.publicnode.com","https://mainnet.gateway.tenderly.co"):
        try:
            header=request(url,"eth_getBlockByNumber",[hex(source["block"]),False])
            if (request(url,"eth_chainId",[])!="0x1" or header["hash"]!=tag["blockHash"]
                    or header["stateRoot"]!=source["state_root"]):
                continue
            probe=simulate_block(url,tag,{},[call(pool,calldata("getReserves()"))],read_rpc=request)
            if probe[0]["status"]!="0x1" or list(decode(["uint112","uint112","uint32"],bytes.fromhex(probe[0]["returnData"][2:]))[:2])!=list(map(int,source["reserves"])):
                continue
            nodes.append(url)
        except (requests.RequestException,ValueError,RuntimeError,KeyError,TypeError):
            continue
    if not nodes:
        raise RuntimeError("no Ethereum archive reproduced canonical hash, state root and pair reserves")
    def archive(fn):
        for attempt in range(3):
            for node in nodes:
                try:
                    return fn(node)
                except (requests.RequestException,RuntimeError,KeyError,TypeError,ValueError) as error:
                    last=error
            time.sleep(2**attempt)
        raise RuntimeError("verified archives could not complete read") from last
    def read(m,p):
        return archive(lambda node:request(node,m,p))
    for contract in source["sources"].values():
        actual=read("eth_getStorageAt",[contract["proxy"],SLOT,tag])
        assert "0x"+actual[-40:]==contract["implementation"]
    for a,expected in source["code_hashes"].items():
        assert "0x"+keccak(bytes.fromhex(read("eth_getCode",[a,tag])[2:])).hex()==expected
    assert read("eth_getCode",[EXEC,tag])=="0x"
    head=read("eth_getBlockByNumber",[hex(source["block"]),False])
    assert head["hash"]==tag["blockHash"] and head["stateRoot"]==source["state_root"]
    deadline=int(head["timestamp"],16)+300
    runtime=RUNTIME.read_text().strip().removeprefix("0x")
    word=lambda n: "0x"+format(n,"064x")

    def measure(wallet):
        sender=wallet["sender"]
        assert read("eth_getCode",[sender,tag])=="0x"
        over={EXEC:{"code":"0x"+runtime,"stateDiff":{word(0):word(int(sender,16)),word(1):word(int(weth,16))}}}
        def tx(to,sig,types=(),args=()):
            return {**call(to,calldata(sig,types,args)),"from":sender}
        def simulate(calls):
            def frame(node):
                reply=simulate_block(node,tag,over,calls,read_rpc=request)
                if any(v.get("status") not in ("0x0","0x1") or not isinstance(v.get("returnData"),str) for v in reply):
                    raise RuntimeError("incomplete simulation results")
                return reply
            return archive(frame)
        def balance(t,a):
            return tx(t,"balanceOf(address)",["address"],[a])
        def allowance(t,spender):
            return tx(t,"allowance(address,address)",["address","address"],[sender,spender])
        def value(v):
            assert v["status"]=="0x1"
            return int(v["returnData"],16)
        def swap(mode,i,n,minimum):
            path=[tokens[i],tokens[1-i]]
            if mode=="router":
                return tx(router,"swapExactTokensForTokens(uint256,uint256,address[],address,uint256)",
                          ["uint256","uint256","address[]","address","uint256"],[n,minimum,path,sender,deadline])
            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                      ["address","uint256","address","uint256","uint256",STEP+"[]"],
                      [path[0],n,path[1],minimum,deadline,[(0,pool,*path,10000,encode(["uint32"],[3000]))]])
        initial=simulate([balance(source["wse"],sender),balance(tokens[1],sender),allowance(tokens[1],router)])
        wse,cash,allow=map(value,initial)
        d=sum(wse>=step*10**18 for step in [100,1000,10000,100000,1000000])
        assert d==wallet["tier"] and wse==int(wallet["wse"])
        sizes=[int(wallet["input"])//2,int(wallet["input"])]
        assert min(cash,allow)>=max(sizes)
        results=[]
        for budget in sizes:
            for i in (1,0):
                prefix=[]
                n=budget
                if i==0:
                    # Fund WBT with an ordinary router swap from this EOA in both alternatives.
                    n=amount_out(budget,int(source["reserves"][1]),int(source["reserves"][0]),d)
                    prefix.append(swap("router",1,budget,n))
                for mode,spender in (("router",router),("v5",EXEC)):
                    before=simulate(prefix+[allowance(tokens[i],spender),tx(pool,"getReserves()")])
                    assert all(r["status"]=="0x1" for r in before)
                    reserves=decode(["uint112","uint112","uint32"],bytes.fromhex(before[-1]["returnData"][2:]))[:2]
                    minimum=amount_out(n,reserves[i],reserves[1-i],d if mode=="router" else 0)
                    present=value(before[-2])
                    prep=[]
                    if present<n:
                        for approved in ([0,n] if present else [n]):
                            prep.append(tx(tokens[i],"approve(address,uint256)",["address","uint256"],[spender,approved]))
                    balances=[balance(tokens[i],sender),balance(tokens[1-i],sender),allowance(tokens[i],spender)]
                    start=prefix+prep+balances
                    deny=swap(mode,i,n,minimum+1)
                    ok=swap(mode,i,n,minimum)
                    replies=simulate(start+[deny]+balances+[ok]+balances+[balance(t,EXEC) for t in tokens])
                    ix=len(start)
                    assert all(r["status"]==("0x0" if k==ix else "0x1") for k,r in enumerate(replies)),(mode,i,d,n,[(k,r.get("error")) for k,r in enumerate(replies) if r["status"]!="0x1"])
                    first=list(map(value,replies[ix-3:ix]))
                    assert list(map(value,replies[ix+1:ix+4]))==first
                    actual=replies[ix+4];after=list(map(value,replies[ix+5:ix+8]))
                    spent,received=first[0]-after[0],after[1]-first[1]
                    assert spent==n and received==minimum and all(value(r)==0 for r in replies[-2:])
                    if mode=="router":
                        reported=decode(["uint256[]"],bytes.fromhex(actual["returnData"][2:]))[0]
                        assert list(reported)==[n,received]
                    results.append(dict(payer=sender,tier=d,mode=mode,input=tokens[i],output=tokens[1-i],cash_budget=str(budget),
                                        requested=str(n),spent=str(spent),received=str(received),min_out=str(minimum),
                                        before=list(map(str,first)),after=list(map(str,after)),rollback=list(map(str,first)),
                                        rejected=replies[ix].get("error"),revert_data=replies[ix]["returnData"],
                                        swap_gas=int(actual["gasUsed"],16),approval_gas=[int(r["gasUsed"],16) for r in replies[len(prefix):len(prefix)+len(prep)]],
                                        prefunding_gas=int(replies[0]["gasUsed"],16) if prefix else 0))
        return results
    rows=[case for wallet in source["rows"] for case in measure(wallet)]
    if len(rows)!=24 or {r["tier"] for r in rows}!={0,1,2}:
        raise RuntimeError("incomplete router/V5 wallet comparison")
    assert tag==json.loads(ETH.read_bytes())["selector"]
    result=dict(chain_id=1,block=source["block"],selector=tag,state_root=source["state_root"],source=source["sources"],
                executor_runtime="0x"+keccak(bytes.fromhex(runtime)).hex(),nodes=nodes,rows=rows)
    Path(out).write_text(json.dumps(result,indent=2)+"\n",encoding="utf8")

if __name__=="__main__":
    run(sys.argv[1])
