"""Reproduce RabbitRouter native cash settlement and the two independent fee floors on Base."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
import requests

from record_base_slipstream_executor import OWNER, ROOT, rpc, simulate_block
from record_oasisswap_v5 import allowance, balance, scalar, tx
from record_deltaswap_base import ENDPOINTS

PAIR = "0x7cfc830448484cdf830625373820241e61ef4acf"
ROUTER = "0xbf250ae227de43dedaf01ccbfd8cc83027efc1e2"
FRIEND = "0x0bd4887f7d41b35cd75dff9ffee2856106f86670"
WETH = "0x4200000000000000000000000000000000000006"
MULTICALL = "0xca11bde05977b3631167028862be2a173976ca11"
RECEIVER = "0x" + keccak(text="BunnySwap archive independent recipient 8453")[-20:].hex()
RUNTIME = {PAIR:"0xa1c0708a77f8ed010599122c1cbe8cecfc02d0d87c380a9872a827e22bff2c82",
           ROUTER:"0x28f604c8fed620b19f66357568f068d92096c3e61b8da23b686be689826595dd"}

def capture(archive, endpoint):
    tag = archive["selector"]
    block = rpc(endpoint, "eth_getBlockByHash", [tag["blockHash"],False])
    if block["stateRoot"] != archive["state_root"] or int(rpc(endpoint,"eth_chainId",[]),16) != 8453:
        raise ValueError("wrong canonical Base state")
    for address, digest in RUNTIME.items():
        code = rpc(endpoint,"eth_getCode",[address,tag])
        if "0x"+keccak(hexstr=code).hex() != digest:
            raise ValueError("protocol runtime mismatch")
    def run(calls):
        return simulate_block(endpoint,tag,{OWNER:{"balance":hex(10**18)}},calls)
    def state():
        return [tx(PAIR,"getReserves()"),tx(PAIR,"feeTier()"),tx(ROUTER,"protocolFee()"),
                balance(FRIEND,OWNER),tx(MULTICALL,"getEthBalance(address)",["address"],[RECEIVER]),
                allowance(FRIEND,OWNER,ROUTER),balance(WETH,PAIR),balance(FRIEND,PAIR)]
    def values(results):
        if any(r["status"]!="0x1" for r in results):
            raise ValueError("failed to read reserves or wallet")
        a,b,_=decode(["uint112","uint112","uint32"],bytes.fromhex(results[0]["returnData"][2:]))
        return (a,b,*map(scalar,results[1:]))
    roles=[tx(ROUTER,"pair()"),tx(ROUTER,"token0()"),tx(ROUTER,"token1()"),tx(PAIR,"token0()"),tx(PAIR,"token1()"),
           tx(PAIR,"isAllowedRouter(address)",["address"],[ROUTER]),tx(PAIR,"owner()"),tx(ROUTER,"owner()")]
    if list(map(scalar,run(roles)))!=[int(x,16) for x in [PAIR,FRIEND,WETH,FRIEND,WETH]]+[1,0,0]:
        raise ValueError("unconfirmed router or mutable fixture")
    deadline=int(block["timestamp"],16)+120
    def apply(prefix,buy,amount):
        before=values(run(prefix+state())[-len(state()):]); rf,rw,lp,protocol=before[:4]
        ri,ro=(rw,rf) if buy else (rf,rw)
        funded=amount-amount*protocol//1000 if buy else amount
        adjusted=funded*(1000-lp)
        gross=adjusted*ro//(ri*1000+adjusted)
        received=gross if buy else gross-gross*protocol//1000
        q=tx(ROUTER,"getAmountOut(uint256,uint256,uint256,uint64)",["uint256","uint256","uint256","uint64"],[funded,ri,ro,lp])
        def swap(floor,expiry):
            if buy:
                return tx(ROUTER,"swapExactETHForTokens(uint256,address,uint256)",["uint256","address","uint256"],[floor,OWNER,expiry],value=amount)
            return tx(ROUTER,"swapExactTokensForETH(uint256,uint256,address,uint256)",["uint256","uint256","address","uint256"],[amount,floor,RECEIVER,expiry])
        calls=prefix+[q,swap(received+1,deadline),swap(received,1)]+state()+[swap(received,deadline)]+state()
        results=run(calls); k=len(prefix)
        if any(r["status"]!="0x1" for r in results[:k]) or scalar(results[k])!=gross or received==0:
            raise ValueError("setup, independent formula or funding failed")
        if any(r["status"]!="0x0" for r in results[k+1:k+3]) or values(results[k+3:k+3+len(state())])!=before:
            raise ValueError("price or deadline did not roll back")
        fill=results[k+3+len(state())]; after=values(results[-len(state()):])
        if scalar(fill)!=received or after[4]-before[4]!=(received if buy else -amount) or after[5]-before[5]!=(0 if buy else received):
            raise ValueError("wallet delta differs from the two-stage price")
        if (after[7]-before[7],after[8]-before[8],after[6])!=((funded,-gross,0) if buy else (-gross,amount,0)):
            raise ValueError("unexpected pool debit, credit or remaining allowance")
        return dict(amount=str(amount),reserves=[str(rf),str(rw)],lp=lp,protocol=protocol,gross=str(gross),received=str(received),
                    min_plus_one=results[k+1]["error"],expired=results[k+2]["error"],gas=int(fill["gasUsed"],16)),prefix+[swap(received,deadline)]
    cases=[]
    for amount in (10**13,10**15,10**16):
        buy,prefix=apply([],True,amount); qty=int(buy["received"])
        approve=tx(FRIEND,"approve(address,uint256)",["address","uint256"],[ROUTER,qty])
        sell,_=apply(prefix+[approve],False,qty)
        cases.append(dict(buy=buy,sell=sell))
    return dict(selector=tag,state_root=block["stateRoot"],endpoint=endpoint,pair=PAIR,router=ROUTER,hashes=RUNTIME,cases=cases)

if __name__=="__main__":
    arg=argparse.ArgumentParser(description=__doc__);arg.add_argument("output",type=Path);args=arg.parse_args()
    evidence=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base_native_v3.json").read_bytes())
    errors=[]
    for endpoint in ENDPOINTS:
        try:
            evidence_result=capture(evidence,endpoint)
            args.output.write_text(json.dumps(evidence_result,indent=2)+"\n",encoding="utf8")
            break
        except (ValueError,RuntimeError,requests.RequestException,KeyError) as err: errors.append((endpoint,str(err)[:240]))
    else: raise RuntimeError(errors)
