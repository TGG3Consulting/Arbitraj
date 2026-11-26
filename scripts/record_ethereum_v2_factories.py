"""Reconstruct five Ethereum V2 registries and measure exact wallet settlement on the pinned block."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, rpc
from record_community_v3_ethereum import posts
from record_v2_chain_pools import measure

FACTORIES = {
    "dooar_ethereum": ("0x1e895bfe59e3a5103e8b7da3897d1f2391476f3c",100,1,"DooarSwapV2: K"),
    "standard_ethereum": ("0x53ac1d1fa4f9f6c604b8b198ce29a50d28cba893",1000,3,"LTR: K"),
    "justmoney-ethereum": ("0xd36aba9ec96523b0a89886c76065852adfe2eb39",1000,3,"Pair: Bad Swap (K)"),
    "verse": ("0xee3e9e46e34a27dc755a63e2849c9913ee1a06e2",1000,3,"empty-revert"),
    "spice_trade_ethereum": ("0x71bcc0bf974aae8cc55d033c17d50486fc77e3ae",1000,None,"UniswapV2: K"),
}
WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
STABLES = ("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","0xdac17f958d2ee523a2206206994597c13d831ec7")
FUNDING = "0x7a250d5630b4cf539739df2c5dacb4c659f2488d"


def digest(raw):
    return "0x"+keccak(raw).hex()


def raw(addr):
    return bytes.fromhex(addr[2:])


def origin(name,factory,c):
    url="https://eth.blockscout.com/api/v2/smart-contracts/"+factory
    response=requests.get(url,timeout=30)
    response.raise_for_status()
    record=response.json()
    code=rpc(c.endpoint,"eth_getCode",[factory,c.selector])
    if not record.get("is_verified") or record["is_changed_bytecode"] or code.lower()!=record["deployed_bytecode"].lower():
        raise ValueError("factory runtime differs from the verified deployment")
    sources=[record["source_code"]]+[f["source_code"] for f in record.get("additional_sources",[])]
    lines=[l.strip() for file in sources for l in file.splitlines()
           if "balance0Adjusted =" in l or "balance1Adjusted =" in l or "getFeeForPair(address(this))" in l]
    if len(lines) < 2:
        raise ValueError("verified invariant source unavailable")
    if name=="dooar_ethereum":
        # Factory's CODECOPY builds the creation code of DooarSwapV2Pair as a byte array.
        compiled=raw(code)
        if compiled[2158:2171].hex()!="612d938061087b8339019056fe":
            raise ValueError("Dooar pair creation CODECOPY changed")
        init=digest(compiled[2171:2171+11667])
    elif name=="verse":
        target,=c.one(factory,"cloneTarget()",returns=["address"])
        init=digest(bytes.fromhex("3d602d80600a3d3981f3363d3d373d3d3d363d73")+raw(target)+bytes.fromhex("5af43d82803e903d91602b57fd5bf3"))
    else:
        getter="INIT_CODE_PAIR_HASH()" if name=="justmoney-ethereum" else "pairCodeHash()"
        result,=c.one(factory,getter,returns=["bytes32"])
        init="0x"+result.hex()
    return dict(url=url,runtime=digest(raw(code)),creation_code_hash=init,invariant=lines)


def directory(name,c,*,config=None,network="ethereum",proof=None,wrapped=WETH,stables=STABLES,funding_router=FUNDING):
    # Distinguish an unavailable historical EVM from a failed token or swap.
    frame,=rpc(c.endpoint,"eth_simulateV1",[{"blockStateCalls":[{"calls":[{
        "to":wrapped,"data":"0x313ce567"}]}],"validation":False},c.selector])
    if (frame["parentHash"]!=c.selector["blockHash"] or frame["calls"][0]["status"]!="0x1"
            or decode(["uint8"],raw(frame["calls"][0]["returnData"]))[0]!=18):
        raise ValueError("archive cannot simulate WETH decimals on the canonical parent")
    factory,den,fee,reason=config or FACTORIES[name]
    proof=proof or origin(name,factory,c)
    total,=c.one(factory,"allPairsLength()",returns=["uint256"])
    addresses=[decode(["address"],x)[0] for x in posts(c,[(factory,"allPairs(uint256)",["uint256"],[i]) for i in range(total)])]
    if not total or len(set(addresses))!=total:
        raise ValueError("incomplete registry")
    operations=[(p,sig,[],[]) for p in addresses for sig in ("token0()","token1()","getReserves()","factory()")]
    values=iter(posts(c,operations))
    rows=[]
    for pool in addresses:
        tokens=[decode(["address"],next(values))[0] for _ in (0,1)]
        reserves=decode(["uint112","uint112","uint32"],next(values))[:2]
        owner,=decode(["address"],next(values))
        expected="0x"+keccak(b'\xff'+raw(factory)+keccak(raw(tokens[0])+raw(tokens[1]))+raw(proof["creation_code_hash"]))[-20:].hex()
        if owner!=factory or expected!=pool or tokens!=sorted(tokens):
            raise ValueError("registry and CREATE2 disagree for "+pool)
        rows.append(dict(pool=pool,tokens=tokens,reserves=list(map(str,reserves))))
    ops=[(factory,"getPair(address,address)",["address","address"],tokens)
         for p in rows for tokens in (p["tokens"],p["tokens"][::-1])]
    checks=iter(posts(c,ops))
    for row in rows:
        if any(decode(["address"],next(checks))[0]!=row["pool"] for _ in (0,1)):
            raise ValueError("getPair failed for "+row["pool"])
    candidates=[]
    for asset in (wrapped,*stables):
        matching=[r for r in rows if asset in r["tokens"] and min(map(int,r["reserves"]))>0]
        matching.sort(key=lambda r:-int(r["reserves"][r["tokens"].index(asset)]))
        candidates.extend((asset,row) for row in matching)
    examples=[]
    for asset,row in candidates:
        pool=row["pool"]
        code=rpc(c.endpoint,"eth_getCode",[pool,c.selector])
        entry=dict(network=network,dex=name,pool=pool,tokens=row["tokens"],factory=factory,
                   runtime_hash=digest(raw(code)),creation_code_hash=proof["creation_code_hash"],
                   verify_pair_boundary=True,invariant_error=reason)
        if fee is None:
            getter=proof.get("fee_getter","getFeeForPair(address)")
            entry.update(fee_target=factory,fee_getter=getter)
            token_args=getter=="getFeeForPair(address,address)"
            actual_fee,=c.one(factory,getter,["address","address"] if token_args else ["address"],
                row["tokens"] if token_args else [pool],["uint256"])
        else:
            actual_fee=fee
        entry["fee_fraction"]=[den,actual_fee]
        if asset!=wrapped:entry.update(funded_asset=asset,funding_router=funding_router)
        try:
            chain=json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())[network]
            if chain["hash"]!=c.selector["blockHash"]:
                raise RuntimeError("executor capture and registry refer to different blocks")
            replay=measure(entry,chain,endpoint=c.endpoint)
        except (ValueError,KeyError) as error:
            examples.append(dict(pool=pool,funded_asset=asset,reason=str(error)[:250]))
            continue
        examples.append(replay)
        break
    return dict(factory=factory,origin=proof,pools=rows,samples=examples)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("market",choices=FACTORIES)
    ap.add_argument("output",type=Path)
    args=ap.parse_args()
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    for endpoint in dict.fromkeys((snap["endpoint"],"https://ethereum.gateway.tenderly.co","https://ethereum.publicnode.com")):
        try:
            cap=Capture(dict(snap,endpoint=endpoint))
            head=rpc(endpoint,"eth_getBlockByHash",[snap["selector"]["blockHash"],False])
            if head["hash"]!=snap["selector"]["blockHash"] or int(head["number"],16)!=snap["block"] or int(rpc(endpoint,"eth_chainId",[]),16)!=1:
                raise ValueError("wrong chain or header")
            result=directory(args.market,cap)
            result.update(selector=snap["selector"],height=snap["block"],state_root=head["stateRoot"],endpoint=endpoint)
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8")
            break
        except (requests.RequestException,ValueError,KeyError) as failure:
            print(endpoint,str(failure)[:220],flush=True)
    else:
        raise ValueError("no canonical archive supplied a full registry")
