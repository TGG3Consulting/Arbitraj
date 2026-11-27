"""Measure the two independent Basin-compatible deployments on their canonical network snapshots."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
from fork_fee_rpc import Reader
from record_base_slipstream_executor import ROOT
from record_llama_base_cl import CanonicalCalls

WELLS = {
    "base": [
        "0x3e11001cfbb6de5737327c59e10afab47b82b5d3",
        "0x3e111115a82df6190e36adf0d552880663a4dbf1",
        "0x3e11226fe3d85142b734abce6e58918d5828d1b4",
        "0x3e1133ac082716ddc3114bbefeed8b1731ea9cb1",
        "0x3e11444c7650234c748d743d8d374fce2ee5e6c9",
    ],
    "ethereum": [
        "0xbea0e11282e2bb5893bece110cf199501e872bad",
        "0xbea0000113b0d182f4064c86b71c315389e4715d",
        "0x1125eac5f713503e2b7cb2299027960ce1aa5d42",
        "0x54c04c9bf5af0bc3096cb0af24c4fa8379a2915e",
        "0x905eafe9434fabacaf10d1490fcd0d1eb9b85fc8",
        "0x8d97775623368f833f8fa82209e220f1c60508ea",
        "0xdf9c4a067279857b463817ef773fe189c77e1686",
    ],
}
SOURCES = {
    "base": ["https://docs.pinto.money/resources/contracts",
             "https://github.com/DefiLlama/dimension-adapters/blob/be9967c3f0d3ccd06c871bab0d16beb3304de719/dexs/pinto/index.ts"],
    "ethereum": ["https://docs.basin.exchange/resources/contracts",
                 "https://github.com/DefiLlama/dimension-adapters/blob/be9967c3f0d3ccd06c871bab0d16beb3304de719/dexs/basin/index.ts"],
}
ZERO = "0x"+"00"*20
IMPL = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"

def capture(network):
    data = ROOT/"tests/data/first_run_dex_2026-11-26"
    head=json.loads((data/(network+".json")).read_bytes())
    reader=Reader(network,head)
    client=CanonicalCalls(reader)
    def one(addr, selector, result, types=(), args=()):
        return client.one(addr,selector,types,args,[result])[0]
    def code(addr):
        raw=reader.one("eth_getCode",[addr,reader.tag])
        if raw=="0x": raise ValueError(f"no code at {addr}")
        return "0x"+keccak(hexstr=raw).hex()
    def optional(f):
        try: return f()
        except ValueError as exc: return {"unavailable":str(exc)[:200]}
    def market(pool):
        roles={}
        def role(k,addr):
            if addr==ZERO:return
            roles[k]={"address":addr,"runtime":code(addr)}
        role("well",pool)
        tokens=one(pool,"tokens()","address[]")
        reserves=one(pool,"getReserves()","uint256[]")
        assert len(tokens)==len(reserves)==2 and tokens[0]!=tokens[1]
        factory=one(pool,"aquifer()","address")
        role("aquifer",factory)
        def registered(a):
            return one(factory,"wellImplementation(address)","address",["address"],[a])
        role("registered",registered(pool))
        target,=decode(["address"],bytes.fromhex(reader.one("eth_getStorageAt",[pool,IMPL,reader.tag])[2:]))
        role("proxy_target",target)
        if target!=ZERO:role("registered_proxy_target",registered(target))
        fn,extra=one(pool,"wellFunction()","(address,bytes)")
        role("function",fn)
        name=optional(lambda:one(fn,"name()","string"))
        version=optional(lambda:one(fn,"version()","string"))
        count=one(pool,"totalSupply()","uint256")
        underlying=[]
        for coin,reserve in zip(tokens,reserves,strict=True):
            underlying.append({"token":coin,"decimals":one(coin,"decimals()","uint8"),"reserve":reserve,
                "balance":one(coin,"balanceOf(address)","uint256",["address"],[pool])})
        quotes=[]
        for i,token in enumerate(underlying):
            reserve=token["reserve"]
            for amount in sorted({1,max(1,reserve//1_000_000),max(1,reserve//1000)}):
                quote=optional(lambda:one(pool,"getSwapOut(address,address,uint256)","uint256",
                    ["address","address","uint256"],[tokens[i],tokens[1-i],amount]))
                quotes.append({"direction":i,"amount":amount,"out":quote})
        return {"roles":roles,"tokens":underlying,"fnData":"0x"+extra.hex(),"fnName":name,
                "fnVersion":version,"supply":count,"quotes":quotes}
    return {"chain":head["chain_id"],"block":head["block"],"selector":reader.tag,"sources":SOURCES[network],
            "wells":{w:market(w) for w in WELLS[network]}}
if __name__=="__main__":
    p=argparse.ArgumentParser()
    p.add_argument("network",choices=list(WELLS));p.add_argument("result",type=Path);args=p.parse_args()
    args.result.write_bytes(json.dumps(capture(args.network),separators=(",",":")).encode())
