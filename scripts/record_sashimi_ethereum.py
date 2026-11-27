"""Bind the Ethereum Sashimi factory, pairs and Router02 to the published SDK and actual ERC20 settlement."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode
from record_balancer_markets import Capture
from record_base_classic_directory import optional
from record_base_slipstream_executor import ROOT, rpc
from record_ethereum_v2_factories import WETH, digest, raw, registry
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH, verified

DATA=ROOT/"tests/data/first_run_dex_2026-11-26"
FACTORY="0xf028f723ed1d0fe01cc59973c49298aa95c57472"
ROUTER="0xe4fe6a45f354e845f954cddee6084603cedb9410"
INIT="0xb465bbe4edb8c9b0da8ff0b2b36ce0065de9fcd5a33f32c6856ea821779c8b72"
SDK="https://github.com/SashimiProject/sashimiswap-sdk/blob/50fb3f0b7fcc7108be957a0616158a952f60ca2e/src/constants.ts"
UI="https://github.com/SashimiProject/sashimi-swap-interface/blob/c58d948d7ee2c51dddd1aa79bdd5b5dd8b9d7441/src/constants/index.ts"
SUBGRAPH="https://github.com/SashimiProject/sashimi-swap-subgraph/blob/41fb7a08a86671b6cb7950fd650362473c791f9d/subgraph.yaml"


def bindings(c):
    first,=c.one(FACTORY,"allPairs(uint256)",["uint256"],[0],["address"])
    factory_url,_,factory=verified(c,FACTORY,"UniswapV2Factory")
    pool_url,meta,pair=verified(c,first,"UniswapV2Pair")
    router_url,router_src,router=verified(c,ROUTER,"UniswapV2Router02")
    init=raw(meta["creationBytecode"]["onchainBytecode"])
    if digest(init)!=INIT or factory.count(init)!=1:
        raise ValueError("pair creation bytes disagree with published SDK or factory")
    if c.one(ROUTER,"factory()",returns=["address"])!=(FACTORY,) or c.one(ROUTER,"WETH()",returns=["address"])!=(WETH,):
        raise ValueError("Router02 factory/WETH mismatch")
    code="".join(meta["sources"][meta["compilation"]["fullyQualifiedName"].split(":")[0]]["content"].split())
    if not all(s in code for s in ["balance0.mul(1000).sub(amount0In.mul(3))","balance1.mul(1000).sub(amount1In.mul(3))","balance0Adjusted.mul(balance1Adjusted)>=uint(_reserve0).mul(_reserve1).mul(1000**2)"]):
        raise ValueError("verified fee or invariant differs")
    router_code="".join(next(v["content"] for k,v in router_src["sources"].items() if k.endswith("UniswapV2Router02.sol")).split())
    if "getTokenInPair(address(this),_token)" not in code or "_pools[pair][token]" not in router_code or "withdrawWithReBalance(token,amount.sub(balance))" not in router_code:
        raise ValueError("Sashimi routed accounting or custody changed")
    return dict(url=factory_url,runtime=digest(factory),creation_code_hash=INIT,verified_pair=pool_url,pair_runtime=digest(pair),
                router=ROUTER,router_runtime=digest(router),router_source=router_url,official=[SDK,UI,SUBGRAPH])


def inventory(c,proof):
    markets=registry(c,FACTORY,INIT)
    balances=iter(optional(c,[(t,"balanceOf(address)",["address"],[m["pool"]]) for m in markets for t in m["tokens"]]))
    held=iter(optional(c,[(ROUTER,"getTokenInPair(address,address)",["address","address"],[m["pool"],t]) for m in markets for t in m["tokens"]]))
    tokens=sorted({t for m in markets for t in m["tokens"]})
    totals=optional(c,[(t,"balanceOf(address)",["address"],[ROUTER]) for t in tokens])
    val=lambda v:None if v is None else str(decode(["uint256"],v)[0])
    for m in markets:
        m["pair_balances"]=[val(next(balances)),val(next(balances))]
        m["router_ledger"]=[val(next(held)),val(next(held))]
    vault,=c.one(ROUTER,"vault()",returns=["address"])
    return dict(factory=FACTORY,origin=proof,pools=markets,vault=vault,router_balances={t:val(v) for t,v in zip(tokens,totals,strict=True)})


def capture(mode,endpoint):
    snap=json.loads((DATA/"ethereum.json").read_bytes())
    c=Capture(dict(snap,endpoint=endpoint))
    header=rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False])
    if int(rpc(endpoint,"eth_chainId",[]),16)!=1 or header["stateRoot"]!=ROOT_HASH or int(header["number"],16)!=snap["block"]:
        raise ValueError("wrong chain or market state")
    proof=bindings(c)
    if mode=="directory":
        data=inventory(c,proof)
    else:
        from record_sashimi_wallets import wallets
        data=inventory(c,proof)
        data["wallets"]=wallets(c,header,data)
    return dict(data,selector=c.selector,height=snap["block"],state_root=ROOT_HASH)


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("mode",choices=("directory","wallet"))
    cli.add_argument("output",type=Path)
    a=cli.parse_args()
    for endpoint in ENDPOINTS:
        try:
            result=capture(a.mode,endpoint)
        except (ValueError,RuntimeError,KeyError,OSError) as e:
            print(endpoint,str(e)[:240],flush=True)
            continue
        payload=(json.dumps(result,separators=(",",":"))+"\n").encode()
        a.output.write_bytes(gzip.compress(payload,mtime=0) if a.output.suffix==".gz" else payload)
        break
    else:
        raise ValueError("canonical Sashimi capture unavailable")
