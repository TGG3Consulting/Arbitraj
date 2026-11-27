"""Bind the nine Base Virtus CL clones and pay the original router and V5 using wallet-owned assets."""
import argparse
import json
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak
from fork_fee_rpc import Reader
from record_llama_base_cl import CanonicalCalls, ROOT, wallet

FACTORY = "0x0e5ab24bebda7e5bb3961f7e9b3532a83ae86b48"
ROUTER = "0xab08ee1f3dc8cbbdb1b781484ecae8d87ecb6ee7"
QUOTER = "0x390f37392923dc61af96d332cf61611d2e5bd997"
IMPL = "0xcd3d85d82137d3094e6f4f7a507d309a2c2639f8"
FEE = "0xd9f77f8b00d42e2a49d82f8115cd5db380550750"
USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"


def capture():
    head = json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    reader = Reader("base",head)
    cap = CanonicalCalls(reader)
    def one(to, sig, ty, args=(), values=()):
        return cap.one(to,sig,args,values,[ty])[0]
    roles = {"factory":FACTORY,"router":ROUTER,"quoter":QUOTER,"implementation":IMPL,"fee_module":FEE}
    code = reader.many(("eth_getCode",[addr,cap.selector]) for addr in roles.values())
    registry = {name: {"address":addr,"bytes":len(raw[2:])//2,"runtime":"0x"+keccak(hexstr=raw).hex()}
                for (name,addr),raw in zip(roles.items(),code,strict=True)}
    assert all(r["bytes"] for r in registry.values())
    assert all(one(a,"factory()","address")==FACTORY for a in (ROUTER,QUOTER,FEE))
    assert one(FACTORY,"poolImplementation()","address")==IMPL and one(FACTORY,"swapFeeModule()","address")==FEE
    runtime = bytes.fromhex("363d3d373d3d3d363d73"+IMPL[2:]+"5af43d82803e903d91602b57fd5bf3")
    init = keccak(bytes.fromhex("3d602d80600a3d3981f3")+runtime)
    count = one(FACTORY,"allPoolsLength()","uint256")
    markets = []
    for i in range(count):
        pool = one(FACTORY,"allPools(uint256)","address",["uint256"],[i])
        a,b = [one(pool,"token"+str(j)+"()","address") for j in range(2)]
        sp = one(pool,"tickSpacing()","int24")
        predicted = keccak(b"\xff"+bytes.fromhex(FACTORY[2:])+keccak(encode(["address","address","int24"],[a,b,sp]))+init)[-20:]
        assert a<b and pool=="0x"+predicted.hex() and one(pool,"factory()","address")==FACTORY
        assert reader.one("eth_getCode",[pool,cap.selector])=="0x"+runtime.hex()
        assert all(one(FACTORY,"getPool(address,address,int24)","address",["address","address","int24"],[x,y,sp])==pool for x,y in ((a,b),(b,a)))
        base = one(FACTORY,"tickSpacingToFee(int24)","uint24",["int24"],[sp])
        custom = one(FEE,"customFee(address)","uint24",["address"],[pool])
        fee = one(pool,"fee()","uint24")
        assert fee==one(FACTORY,"getSwapFee(address)","uint24",["address"],[pool])==(0 if custom==420 else custom or base)
        markets.append(dict(pool=pool,tokens=[a,b],key=sp,spacing=sp,current_fee=fee,base_fee=base,custom_fee=custom,
                            liquidity=one(pool,"liquidity()","uint128"),balances=[str(one(t,"balanceOf(address)","uint256",["address"],[pool])) for t in (a,b)]))
    book=dict(name="virtus-cl",chain_id=8453,height=head["block"],selector=cap.selector,roles=registry,markets=markets,
              key_type="int24",init="0x"+init.hex(),counter=USDC,native_wire="slipstream",source="https://liquidity.virtus-protocol.com/")
    return wallet(reader,cap,book)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    parser.parse_args().output.write_bytes(json.dumps(capture(),separators=(",",":")).encode())
