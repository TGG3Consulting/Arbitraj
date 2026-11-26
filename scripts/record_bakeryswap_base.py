"""Replay the same funded amounts through BakerySwap's three-argument pair and its own router."""
from eth_abi import decode
from eth_utils import keccak
import requests

from record_base_slipstream_executor import OWNER, call, calldata, rpc, simulate_block
from record_v2_chain_pools import minimum

ROUTER = "0x10cc46af6964fba2b7266ddf2cd8a8585b71a210"
WETH = "0x4200000000000000000000000000000000000006"


def inspect(cap, directory, header, router=ROUTER, all_candidates=False):
    factory=directory["factory"]
    target=f"https://sourcify.dev/server/v2/contract/8453/{router}"
    src=requests.get(target,params={"fields":"compilation,sources,runtimeBytecode.onchainBytecode"},timeout=25)
    src.raise_for_status()
    code=rpc(cap.endpoint,"eth_getCode",[router,cap.selector])
    if src.json()["runtimeBytecode"]["onchainBytecode"].lower()!=code.lower():
        raise ValueError("archive router differs from verified source")
    if cap.one(router,"factory()",returns=["address"])!=(factory,) or cap.one(router,"WETH()",returns=["address"])!=(WETH,):
        raise ValueError("router factory or wrapped token differs from settings")
    deadline=int(header["timestamp"],16)+120
    def tx(target,signature,types=(),args=()):
        return call(target,calldata(signature,types,args))
    def payout(a,b,n,m):
        return tx(router,"swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
                  ["uint256","uint256","address[]","address","uint256"],[n,m,[a,b],OWNER,deadline])
    def simulate(calls):
        return simulate_block(cap.endpoint,cap.selector,{OWNER:{"balance":hex(100*10**18)}},calls)
    def balance(token,who=OWNER):return tx(token,"balanceOf(address)",["address"],[who])
    def approve(token,n):return tx(token,"approve(address,uint256)",["address","uint256"],[router,n])
    def check(calls):
        for i,r in enumerate(calls):
            if r["status"]!="0x1":
                raise ValueError(f"call {i} {r.get('error')}")
    def val(reply):return decode(["uint256"],bytes.fromhex(reply["returnData"][2:]))[0]
    def leg(prefix,a,b,amount,expected):
        prep=prefix+[approve(a,amount),balance(a),balance(b)]
        q=tx(router,"getAmountsOut(uint256,address[])",["uint256","address[]"],[amount,[a,b]])
        order=payout(a,b,amount,expected)
        calls=simulate(prep+[q,payout(a,b,amount,expected+1),order,balance(a),balance(b),
                  tx(a,"allowance(address,address)",["address","address"],[OWNER,router])])
        j=len(prep);check(calls[:j+1]);check(calls[j+2:])
        quoted,=decode(["uint256[]"],bytes.fromhex(calls[j]["returnData"][2:]))
        paid=val(calls[j-2])-val(calls[j+3]);received=val(calls[j+4])-val(calls[j-1])
        if list(quoted)!=[amount,expected] or (paid,received,val(calls[-1]))!=(amount,expected,0) or calls[j+1]["status"]!="0x0":
            raise ValueError("router quote, strict minimum or wallet delta mismatch")
        return {"amount":str(amount),"quoted":str(expected),"spent":str(paid),"received":str(received),
                "min_plus_one":calls[j+1]["error"],"gas":int(calls[j+2]["gasUsed"],16)},prep+[order]
    candidates=[p for p in directory["pools"] if WETH in p["tokens"] and min(map(int,p["reserves"]))>0]
    candidates.sort(key=lambda p:-int(p["reserves"][p["tokens"].index(WETH)]))
    tested=[]
    for pool in candidates:
        idx=pool["tokens"].index(WETH); rin,rout=[int(pool["reserves"][i]) for i in (idx,1-idx)]
        other=pool["tokens"][1-idx];samples=[]
        try:
            sizes=sorted({min(10**15,rin//10000),min(10**16,rin//500)})
            if len(sizes)!=2 or not sizes[0]:raise ValueError("insufficient reserves for two amounts")
            for amount in sizes:
                purchased=minimum(amount,rin,rout,3000); sold=minimum(purchased,rout-purchased,rin+amount,3000)
                if not sold:raise ValueError("zero output")
                funding=[call(WETH,calldata("deposit()"),amount)]
                buy,prefix=leg(funding,WETH,other,amount,purchased)
                sell,_=leg(prefix,other,WETH,purchased,sold)
                samples.append({"buy":buy,"sell":sell})
        except ValueError as e:
            tested.append({"pool":pool["pool"],"error":str(e)[:220]})
            continue
        tested.append({"pool":pool["pool"],"cases":samples})
        if not all_candidates:
            break
    return {"router":router,"runtime":"0x"+keccak(bytes.fromhex(code[2:])).hex(),"source":target,"samples":tested}
