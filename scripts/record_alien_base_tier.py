"""Fund both sides of the omitted WETH/AERO fee tier through the deployed pool ABI and V5."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode

from record_balancer_markets import Capture,posts
from record_balancer_v1 import rpc
from record_base_slipstream_executor import EXEC,OWNER,ROOT,RUNTIME,STEP,simulate_block
from record_bitswap_ethereum import balance,tx

FACTORY="0x0fd83557b2be93617c9c1c1b6fd549401c74558c"
WETH="0x4200000000000000000000000000000000000006"
AERO="0x940181a94a35a4569e4529a3cdfb74e38fd98631"
SOURCE="https://github.com/alienbase-xyz/docs/blob/c6cee73b54bd92104b879e6ca9a02a6d51b80276/contracts.md"
UINT="uint256"


def record(url):
    anchor=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    at=anchor["selector"]
    header=rpc(url,"eth_getBlockByNumber",[hex(anchor["block"]),False])
    if rpc(url,"eth_chainId",[])!="0x2105" or header["hash"]!=at["blockHash"]:
        raise ValueError("wrong Base chain or block")
    ts=int(header["timestamp"],16)
    c=Capture(dict(anchor,endpoint=url))
    spacing,=c.one(FACTORY,"feeAmountTickSpacing(uint24)",["uint24"],[750],["int24"])
    pools=posts(c,[(FACTORY,"getPool(address,address,uint24)",["address","address","uint24"],[a,b,750])
                   for a,b in [(WETH,AERO),(AERO,WETH)]])
    pool=decode(["address"],pools[0])[0]
    if pools[0]!=pools[1] or int(pool,16)==0:
        raise ValueError("asymmetric or missing factory pool")
    fields=posts(c,[(pool,getter,[],[]) for getter in ["factory()","token0()","token1()","fee()","tickSpacing()","liquidity()"]])
    roles=[decode(["address"],v)[0] for v in fields[:3]]
    if roles[0]!=FACTORY or set(roles[1:])!={WETH,AERO} or int.from_bytes(fields[3],"big")!=750 or int.from_bytes(fields[4],"big",signed=True)!=spacing:
        raise ValueError("registered pool disagrees with original factory")
    def word(a):
        return "0x"+format(int(a,16),"064x")
    overrides={OWNER:{"balance":hex(10**18)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
               "stateDiff":{word("0"):word(OWNER),word("1"):word(WETH)}}}
    if any(rpc(url,"eth_getCode",[a,at])!="0x" for a in overrides):
        raise ValueError("test address already in use")
    def allowance(asset,n):
        return tx(asset,"approve(address,uint256)",["address",UINT],[EXEC,n])
    def order(src,dst,amount,minimum,end):
        step=(1,pool,src,dst,10_000,b"")
        request=tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address",UINT,"address",UINT,UINT,STEP+"[]"],[src,amount,dst,minimum,end,[step]])
        request["gas"]=hex(8_000_000)
        return request
    def measured(prep,src,dst,n):
        balances=[balance(asset,who) for who in (OWNER,EXEC) for asset in (src,dst)]
        def run(calls):
            f=simulate_block(url,at,overrides,prep+balances+calls+balances,rpc)
            if any(v["status"]!="0x1" for v in f[:len(prep)+4]+f[-4:]):
                raise ValueError("cash/funding")
            before,after=([int(v["returnData"],16) for v in q] for q in (f[len(prep):len(prep)+4],f[-4:]))
            return f[len(prep)+4:-4],before[0]-after[0],after[1]-before[1],before[2:]==after[2:]
        result,spent,out,clean=run([order(src,dst,n,1,ts+600)])
        if result[0]["status"]!="0x1":
            if spent or out or not clean:
                raise ValueError(("reverted swap changed cash",src,n))
            return dict(source=src,target=dst,requested=str(n),status="reverted",error=result[0].get("error"),
                        gas=int(result[0]["gasUsed"],16),spent="0",received="0")
        if spent!=n or out<=0 or not clean:
            raise ValueError(("missing or wrong cash",src,n))
        results,again,net,clean=run([order(src,dst,n,out+1,ts+600),order(src,dst,n,out,0),order(src,dst,n,out,ts+600)])
        if [v["status"] for v in results]!=["0x0","0x0","0x1"] or (again,net)!=(spent,out) or not clean:
            raise ValueError("minimum/deadline or recipient mismatch")
        return dict(source=src,target=dst,requested=str(n),status="paid",spent=str(spent),received=str(out),gas=int(results[-1]["gasUsed"],16))
    cash=[]
    for amount in (10**12,10**14,10**15):
        prep=[tx(WETH,"deposit()",value=amount),allowance(WETH,amount)]
        buy=measured(prep,WETH,AERO,amount)
        if buy["status"]=="reverted":
            cash.append(dict(buy=buy,sell_after_buy=None))
            continue
        net=int(buy["received"])
        reverse=prep+[order(WETH,AERO,amount,net,ts+600),allowance(AERO,net)]
        sell=measured(reverse,AERO,WETH,net)
        cash.append(dict(buy=buy,sell_after_buy=sell))
    if not any(r["sell_after_buy"] and r["sell_after_buy"]["status"]=="paid" for r in cash):
        raise ValueError("no original round trip settled")
    return dict(network="base",block=anchor["block"],selector=at,source=SOURCE,factory=FACTORY,pool=pool,
                fee=750,spacing=spacing,tokens=roles[1:],liquidity=str(int.from_bytes(fields[5],"big")),round_trips=cash)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("output",type=Path)
    args=p.parse_args()
    errors=[]
    for url in ["https://base.drpc.org","https://base.gateway.tenderly.co","https://mainnet.base.org"]:
        try:
            result=record(url)
            break
        except (requests.RequestException,RuntimeError,ValueError,KeyError) as e:
            errors.append((url,str(e)[:250]))
    else:
        raise ValueError(errors)
    args.output.write_bytes(json.dumps(result,separators=(",",":")).encode())
