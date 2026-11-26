"""Каталоги CitadelSwap, Kim V2 и LeetSwap с обеими формами пары на одном каноническом блоке Base."""
import argparse
import gzip
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import requests
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, calldata, rpc

MARKETS = {
    "citadelswap": ("0xbe720274c24b5ec773559b8c7e28c2503dac7645","0x3fe49194d3cf9a7005203468073a32d764fb86b6"),
    "kim-v2-base": ("0x14658340d7d1c112b62509bbf449be1897e8de01","0x4ca28220e0caea31bed133b3f22e1317dfeb79ab"),
    "leetswap-base": ("0x169c06b4cfb09bfd73a81e6f2bb1eb514d75bb19","0x83097b68027239c89576825bbb808ac405bf1c1e"),
}
MULTI = "0xca11bde05977b3631167028862be2a173976ca11"


def verified(source_dir, market, role, address):
    if source_dir:
        return json.loads((source_dir / (market+"-"+role+".json")).read_bytes())
    url=f"https://sourcify.dev/server/v2/contract/8453/{address}"
    response=requests.get(url,params={"fields":"compilation,sources,runtimeBytecode.onchainBytecode,creationBytecode.onchainBytecode,match"},timeout=30)
    response.raise_for_status()
    return response.json()


def batches(evidence, calls, allow_failure=False):
    def page(chunk):
        c=Capture(evidence)
        packed=[(a,True,bytes.fromhex(calldata(method,types,args)[2:])) for a,method,types,args in chunk]
        results,=c.one(MULTI,"aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[packed],["(bool,bytes)[]"])
        if len(results)!=len(chunk) or (not allow_failure and not all(ok for ok,_ in results)):
            raise ValueError("canonical multicall incomplete")
        return [body if ok else b'' for ok,body in results]
    with ThreadPoolExecutor(3) as exe:
        return [out for group in exe.map(page,[calls[i:i+192] for i in range(0,len(calls),192)]) for out in group]


def capture(name,endpoint,source_dir):
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    ref=dict(endpoint=endpoint,selector=snap["selector"])
    c=Capture(ref)
    header=rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False])
    if int(rpc(endpoint,"eth_chainId",[]),16)!=8453 or header["hash"]!=c.selector["blockHash"] or int(header["number"],16)!=snap["block"]:
        raise ValueError("wrong chain or head")
    factory,example=MARKETS[name]
    source=verified(source_dir,name,"factory",factory)
    pair=verified(source_dir,name,"pool",example)
    contracts={}
    for addr,data in [(factory,source),(example,pair)]:
        code=rpc(endpoint,"eth_getCode",[addr,c.selector]).lower()
        if code!=data["runtimeBytecode"]["onchainBytecode"].lower() or code=="0x":
            raise ValueError("verified source does not match canonical runtime")
        contracts[addr]=dict(bytes=(len(code)-2)//2,keccak256="0x"+keccak(bytes.fromhex(code[2:])).hex())
    init=keccak(bytes.fromhex(pair["creationBytecode"]["onchainBytecode"].removeprefix("0x")))
    is_leet=name=="leetswap-base"
    options={}
    if is_leet:
        actual,=c.one(factory,"pairCodeHash()",returns=["bytes32"])
        if init!=actual: raise ValueError("wrong verified creation code")
        for sig,typ in [("isPaused()","bool"),("tradingFeesOracle()","address")]:
            options[sig],=c.one(factory,sig,returns=[typ])
    total,=c.one(factory,"allPairsLength()",returns=["uint256"])
    pools=[decode(["address"],x)[0] for x in batches(ref,[(factory,"allPairs(uint256)",["uint256"],[i]) for i in range(total)])]
    if example not in pools or len(set(pools))!=total:raise ValueError("registry conflict")
    props=("token0()","token1()","factory()","stable()" if is_leet else "stableSwap()","getReserves()")
    vals=iter(batches(ref,[(p,s,[],[]) for p in pools for s in props]))
    markets=[]
    for p in pools:
        t0,t1,creator=[decode(["address"],next(vals))[0] for _ in range(3)]
        stable,=decode(["bool"],next(vals))
        reserves=decode(["uint256"]*(3 if is_leet else 4),next(vals))
        salt=keccak(bytes.fromhex(t0[2:]+t1[2:])+(bytes([stable]) if is_leet else b""))
        expected="0x"+keccak(b"\xff"+bytes.fromhex(factory[2:])+salt+init)[-20:].hex()
        if creator!=factory or t0>=t1 or p!=expected:raise ValueError("CREATE2/market identity mismatch: "+p)
        m=dict(pool=p,tokens=[t0,t1],stable=stable,reserves=[str(x) for x in reserves[:2]])
        if not is_leet: m["fee_percent"]=list(reserves[2:])
        markets.append(m)
    ops=[]
    for m in markets:
        for ts in (m["tokens"],list(reversed(m["tokens"]))):
            ops.append((factory,"getPair(address,address,bool)" if is_leet else "getPair(address,address)",
                        ["address","address"]+(["bool"] if is_leet else []),ts+([m["stable"]] if is_leet else [])))
    checks=iter(batches(ref,ops))
    for m in markets:
        if any(decode(["address"],next(checks))[0]!=m["pool"] for _ in range(2)):
            raise ValueError("two-way lookup disagrees")
    if is_leet:
        ops=[(t,"balanceOf(address)",["address"],[m["pool"]]) for m in markets for t in m["tokens"]]
        funds=iter(batches(ref,ops,allow_failure=True))
        for m in markets:
            m["balances"]=[str(int.from_bytes(data,"big")) if len(data)==32 else None for data in (next(funds),next(funds))]
    return dict(chain=8453,selector=c.selector,height=snap["block"],timestamp=int(header["timestamp"],16),state_root=header["stateRoot"],
                factory=factory,pair_init_hash="0x"+init.hex(),contracts=contracts,settings=options,markets=markets,endpoint=endpoint)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("market",choices=MARKETS)
    parser.add_argument("output",type=Path)
    parser.add_argument("--source-dir",type=Path)
    parser.add_argument("--endpoint",default="https://base-rpc.publicnode.com")
    args=parser.parse_args()
    result=capture(args.market,args.endpoint,args.source_dir)
    data=(json.dumps(result,separators=(",",":"))+"\n").encode()
    args.output.write_bytes(gzip.compress(data,mtime=0) if args.output.suffix==".gz" else data)
