"""Ambient Ethereum: compare CrocImpact, original userCmd(1), and actual wallet balances.
Official migration and lens: https://docs.ambient.finance/developers/dex-contract-interface/swaps/hot-path-swap-migration
https://docs.ambient.finance/developers/query-contracts/crocimpact-contract
"""
import json
import sys
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
import requests

from record_aktionariat_wallet import FUND, SWAP, WETH, approve
from record_balancer_markets import Capture, ZERO, posts
from record_base_slipstream_executor import OWNER, ROOT, rpc, simulate_block
from record_bitswap_ethereum import balance, tx

DEX = "0xaaaaaaaaa24eeeb8d57d431224f73832bc34f688"
QUERY = "0xca00926b6190c2c59336e73f02569c356d7b6b56"
IMPACT = "0x3e3edd3ed7621891e574e5d7f47b1f30a994c0d0"
MULTI = "0xca11bde05977b3631167028862be2a173976ca11"
USDC = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
PAIR = ["address","address","uint256"]
SPEC = ["uint8","uint16","uint8","uint16","uint8","uint8","uint8"]
QUOTE = PAIR+["bool","bool","uint128","uint16","uint128"]
LOW, HIGH = 65538, 21267430153580247136652501917186561138-1
INDEX = ROOT/"tests/data/first_run_dex_2026-11-26/ambient_ethereum_index.json"
POOLS = ["queryPrice","queryLiquidity","queryPoolParams"]


def name(fn, fields):
    return fn+"("+",".join(fields)+")"


def acquire(url):
    chain=json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())["ethereum"]
    tag={"blockHash":chain["hash"],"requireCanonical":True}
    header=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
    assert int(rpc(url,"eth_chainId",[]),16)==1 and int(header["number"],16)==chain["block"] and header["hash"]==chain["hash"]
    end=int(header["timestamp"],16)+900
    c=Capture(dict(endpoint=url,selector=tag))
    for lens in [QUERY,IMPACT]:
        assert c.one(lens,"dex_()",(),(),["address"])[0]==DEX
    # Mainnet singleton delegates callpath 1 through proxyPaths_[1] at storage slot 2.
    # Include the knockout path crossed by swaps as well as the core paths. Governance can replace a path without changing the singleton code.
    paths={}
    for slot in [*range(1,7),3501]:
        value=c.one(DEX,"readSlot(uint256)",["uint256"],[slot],["uint256"])[0]
        assert 0<value<2**160
        paths[str(slot-1)]="0x"+value.to_bytes(20,"big").hex()
    roles={}
    for role,a in {"dex":DEX,"query":QUERY,"impact":IMPACT,**paths}.items():
        code=bytes.fromhex(rpc(url,"eth_getCode",[a,tag])[2:])
        assert code
        roles[role]={"address":a,"hash":"0x"+keccak(code).hex()}
    idx=json.loads(INDEX.read_bytes())
    markets=idx["markets"]
    ops=[(QUERY,name(fn,PAIR),PAIR,m) for m in markets for fn in POOLS]
    result=posts(c,ops)
    states=[]
    for i,(a,b,idx_num) in enumerate(markets):
        vals=result[i*3:i*3+3]
        states.append(dict(base=a,quote=b,index=idx_num,price=int.from_bytes(vals[0],"big"),
                           liquidity=int.from_bytes(vals[1],"big"),parameters=decode(SPEC,vals[2])))
    witnessed=[]
    for a,b in [(ZERO,USDC),(USDC,USDT)]:
        row=next(s for s in states if (s["base"],s["quote"],s["index"])==(a,b,420))
        assert row["price"] and row["parameters"][0]==1
        for buy in [True,False]:
            for exact_in in [True,False]:
                witnessed.append(pay(c,a,b,buy,exact_in,end))
    return dict(head=tag,height=chain["block"],roles=roles,states=states,payments=witnessed,
                index_source=idx["source"],index_date=idx["observed_utc"])


def pay(c,a,b,buy,exact_in,end):
    base_qty=buy==exact_in
    source,target=(a,b) if buy else (b,a)
    fixed=a if base_qty else b
    quantity=10**15 if fixed==ZERO else 1_000_000
    args=[a,b,420,buy,base_qty,quantity,0,HIGH if buy else LOW]
    imp=name("calcImpact",QUOTE)
    def terms(bound):
        packet=encode(QUOTE+["uint128","uint8"],args+[bound,0])
        return tx(DEX,"userCmd(uint16,bytes)",["uint16","bytes"],[1,packet],spent if source==ZERO else 0)
    flows=c.one(IMPACT,imp,QUOTE,args,["int128","int128","uint128"])
    spent,received=(flows[0],-flows[1]) if buy else (flows[1],-flows[0])
    assert spent>0 and received>0
    # Preserve the actual spent and received amounts independently of the requested fixed quantity.
    cap=received if exact_in else spent
    prep=[]
    if source!=ZERO:
        seed=10**17
        market=(WETH,source,500,OWNER,end,seed,spent,0)
        prep=[tx(WETH,"deposit()",value=seed),approve(WETH,FUND,seed),
              tx(FUND,"exactInputSingle("+SWAP+")",[SWAP],[market]),approve(source,DEX,spent)]
    def bal(t):
        return tx(MULTI,"getEthBalance(address)",["address"],[OWNER]) if t==ZERO else balance(t,OWNER)
    balances=[bal(source),bal(target)]
    strict=terms(cap+1 if exact_in else cap-1)
    calls=prep+balances+[strict]+balances+[terms(cap)]+balances
    results=simulate_block(c.endpoint,c.selector,{OWNER:{"balance":hex(2*10**18)}},calls)
    n=len(prep)
    failed=[(i,r.get("error")) for i,r in enumerate(results) if r["status"]!="0x1"]
    if [i for i,_ in failed]!=[n+2]:
        raise ValueError(("unexpected original outcome",source,target,buy,exact_in,spent,received,failed))
    def amounts(i):
        return [int(r["returnData"],16) for r in results[i:i+2]]
    before,middle,after=amounts(n),amounts(n+3),amounts(n+6)
    assert before==middle and [middle[0]-after[0],after[1]-middle[1]]==[spent,received],(source,target,before,middle,after,spent,received)
    out=results[n+5]
    raw=decode(["bytes"],bytes.fromhex(out["returnData"][2:]))[0]
    assert decode(["int128","int128"],raw)==flows[:2]
    return dict(source=source,target=target,exact_input=exact_in,requested=str(quantity),spent=str(spent),received=str(received),
                price_after=str(flows[2]),gas=int(out["gasUsed"],16),original=terms(cap),logs=out["logs"],strict=failed[0][1])


if __name__=="__main__":
    for url in ["https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"]:
        try:
            report=acquire(url)
            break
        except (requests.RequestException,RuntimeError) as exc:
            print(url,str(exc)[:180],flush=True)
    else:
        raise RuntimeError("canonical Ambient node unavailable")
    Path(sys.argv[1]).write_text(json.dumps(report,separators=(",",":")),encoding="utf8")
