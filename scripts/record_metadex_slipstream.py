"""Capture pinned Aero MetaDEX CL contracts, hooks, actual modules and swaps for one network.

Published versions provide the addresses. Sourcify source and bytecode are compared to the
repository commit and state at the canonical hash. Writes one per-chain recording.
"""
import hashlib
import json
import sys
from pathlib import Path

import requests
from eth_utils import keccak
from eth_abi import decode
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, rpc, EXEC
from record_metadex_swaps import run_swaps
from record_metadex_fee import fees

CL = "f99f2095918cab69eafeffa2c3d0a8762de737b7"
CORE = "2ce1816b20948c26a2eec52419c9eccac3857136"
BASE = "https://raw.githubusercontent.com/dromos-labs/"
DIR = ROOT/"tests/data/first_run_dex_2026-11-26"
ZERO = "0x"+"00"*20
TOKENS = {"base": ("0x4200000000000000000000000000000000000006","0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"),
          "ethereum": ("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2","0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")}


def fetch(url):
    r = requests.get(url, timeout=28)
    r.raise_for_status()
    return r


def source(e, address, repo_path, name):
    verified = f"https://sourcify.dev/server/v2/contract/{e['chain_id']}/{address}?fields=compilation,sources,runtimeBytecode.onchainBytecode"
    result = fetch(verified).json()
    chain_code = rpc(e["endpoint"], "eth_getCode", [address, e["selector"]])
    ref = BASE+repo_path
    text = fetch(ref).text
    matched = [s["content"] for k,s in result["sources"].items() if k.endswith("/"+name+".sol") or k==name+".sol"]
    if (result["runtimeMatch"] not in ("match","exact_match") or result["compilation"]["name"] != name
        or result["runtimeBytecode"]["onchainBytecode"].lower() != chain_code.lower() or matched != [text]):
        raise ValueError(("deployed source/bytecode mismatch", name))
    return dict(address=address, contract=name, official=ref, verified=verified,
        source_sha256=hashlib.sha256(text.encode()).hexdigest(), code_keccak="0x"+keccak(bytes.fromhex(chain_code[2:])).hex())


def record(network):
    e = json.loads((DIR/f"{network}.json").read_bytes())
    url = BASE+f"metadex-slipstream-public/{CL}/deployment-addresses/cl-{network}.json"
    d = fetch(url).json()
    c = Capture(e)
    header = rpc(e["endpoint"],"eth_getBlockByHash",[e["selector"]["blockHash"],False])
    if int(rpc(e["endpoint"],"eth_chainId",[]),16)!=e["chain_id"] or int(header["number"],16)!=e["block"] or header["hash"]!=e["selector"]["blockHash"]:
        raise ValueError("chain or header mismatch")
    factory, hook, quoter = (d[k].lower() for k in ("clFactory","dynamicSwapFeeHook","metaquoter"))
    module, = c.one(hook,"mevTaxModule()",returns=["address"])
    if module != d.get("clMevTaxModule",ZERO).lower():
        raise ValueError("unrecognized MEV tax module")
    sources = [source(e,hook,f"metadex-public/{CORE}/V3/src/hooks/dynamic/DynamicSwapFeeHook.sol","DynamicSwapFeeHook")]
    if module != ZERO:
        sources.append(source(e,module,f"metadex-public/{CORE}/V3/src/fees/PriorityFeeMevTaxModule.sol","PriorityFeeMevTaxModule"))
        for f in ("priorityFeeMultiplier()", "minThreshold()"):
            c.one(module,f,returns=["uint256"])
    srepo = f"metadex-slipstream-public/{CL}/contracts/"
    for k,path,name in [("clFactory","core/CLFactory.sol","CLFactory"),("clPoolImplementation","core/CLPool.sol","CLPool"),
                        ("clPoolNonSwapPath","core/libraries/CLPoolNonSwapPath.sol","CLPoolNonSwapPath"),
                        ("metaquoter","periphery/lens/Metaquoter.sol","Metaquoter")]:
        sources.append(source(e,d[k],srepo+path,name))
    for f in ("defaultFeeCap()", "secondsAgo()", "defaultScalingFactor()"):
        c.one(hook,f,returns=["uint256"])
    discounts, = c.one(factory,"discountRegistry()",returns=["address"])
    weth,usdc=TOKENS[network]
    tiers, = c.one(factory,"tickSpacings()",returns=["int24[]"])
    pools=[]
    stables = [usdc]
    if network == "ethereum":
        stables.append("0xdac17f958d2ee523a2206206994597c13d831ec7")
    for stable in stables:
        for spacing in tiers:
            pool, = c.one(factory,"getPool(address,address,int24)",["address","address","int24"],[weth,stable,spacing],["address"])
            if pool==ZERO:continue
            if c.one(factory,"getPoolSwapHook(address)",["address"],[pool],["address"])[0]!=hook:raise ValueError("different hook")
            if c.one(pool,"factory()",returns=["address"])[0]!=factory:raise ValueError("different factory")
            clone=rpc(e["endpoint"],"eth_getCode",[pool,e["selector"]])
            if clone.lower() != "0x363d3d373d3d3d363d73"+d["clPoolImplementation"][2:].lower()+"5af43d82803e903d91602b57fd5bf3":raise ValueError(("unrecognized clone",clone[:110]))
            for f in ("token0()","token1()","fee()","liquidity()","slot0()","tickSpacing()"): c.batch([(pool,f,[],[])])
            c.one(factory,"tickSpacingToFee(int24)",["int24"],[spacing],["uint24"])
            c.one(hook,"dynamicFeeConfig(address)",["address"],[pool],["uint24","uint24","uint64","bool","uint24"])
            for caller in (EXEC,quoter,ZERO):
                if discounts!=ZERO:c.one(discounts,"getDiscount(address,address)",["address","address"],[pool,caller],["uint24"])
            liquidity = c.one(pool,"liquidity()",returns=["uint128"])[0]
            pools.append(dict(pool=pool,spacing=spacing,quoter=quoter,clone=clone,stable=stable,liquidity=str(liquidity)))
    profile = json.loads((ROOT/"config/first_run_dex.json").read_bytes())
    router=next(x["settings"]["dex.router"] for x in profile if x["network"]==network and x["id"] in ("uniswap-v2-base","uniswap_v2"))
    parent=rpc(e["endpoint"],"eth_getBlockByHash",[header["parentHash"],False])
    interval=int(header["timestamp"],16)-int(parent["timestamp"],16)
    # The EVM timestamp has one-second resolution; consecutive Base blocks may have equal timestamps.
    step=max(1,interval)
    out=dict(network=network,chain_id=e["chain_id"],selector=e["selector"],block=e["block"],endpoint=e["endpoint"],
        timestamp=header["timestamp"],parent_timestamp=parent["timestamp"],slot_seconds=step,manifest=url,factory=factory,hook=hook,module=module,quoter=quoter,
        sources=sources,tick_spacings=list(tiers),reads=c.calls,pools=pools,weth=weth,usdc=usdc)
    for pool in pools:
        if int(pool["liquidity"]) > 0:
            pool["head_fee"] = fees(e,pool["pool"],hook,factory,quoter,int(header["timestamp"],16),pool["spacing"],2*step)
    out["v5"]=run_swaps(e,pools,weth,usdc,router,header,step)
    (DIR/f"metadex_{network}.json").write_text(json.dumps(out,indent=2)+"\n",encoding="utf8",newline="\n")


if __name__=="__main__":
    record(sys.argv[1])
