"""Оплачивает обычный AngstromAdapter в архивном блоке с подлинной публичной attestation и собственным тестовым входом."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

from record_angstrom_ethereum import ADAPTER, GATEWAY, HOOK, MANAGER, rpc
from record_balancer_markets import Capture
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, simulate_block
from record_bitswap_ethereum import tx
from record_diamondswap_base import direct_wallet

WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
UNISWAP_V3 = "0x1f98431c8ad98523631ae4a59f267346ea31f984"
KEY = "(address,address,uint24,int24,address)"
ATTEST = "(uint64,bytes)"
DIRECTORY = ROOT / "tests/data/first_run_dex_2026-11-26"


def v5(source, target, quantity, minimum, end, step):
    return tx(EXEC, "execute(address,uint256,address,uint256,uint256," + STEP + "[])",
              ["address","uint256","address","uint256","uint256",STEP+"[]"],
              [source,quantity,target,minimum,end,[step]])


def record(url):
    directory=json.loads(gzip.decompress((DIRECTORY/"angstrom_ethereum_directory.json.gz").read_bytes()))
    witness=json.loads((DIRECTORY/"angstrom_public_attestation.json").read_bytes())
    message=next(m for m in witness["messages"] if len(m["data"])==172)
    height=message["block"]
    head=rpc(url,"eth_getBlockByNumber",[hex(height),False])
    at=dict(blockHash=head["parentHash"],requireCanonical=True)
    if head["hash"]!=message["header"]["hash"] or rpc(url,"eth_chainId",[])!="0x1":
        raise ValueError("Wrong chain, attestation block or canonical parent")
    c=Capture(dict(endpoint=url,selector=at))
    stamp=int(head["timestamp"],16)
    for address in (OWNER,EXEC):
        if rpc(url,"eth_getCode",[address,at])!="0x":raise ValueError("Test address occupied")
    for address,expected in directory["runtime_hashes"].items():
        code=bytes.fromhex(rpc(url,"eth_getCode",[address,at])[2:])
        if "0x"+keccak(code).hex()!=expected:raise ValueError("Market deployment code changed")
    data=bytes.fromhex(message["data"][2:])
    node="0x"+data[:20].hex()
    good=[(height,data)]
    controls=[]
    payments=[]
    word=lambda x:"0x"+format(x,"064x")
    overrides={OWNER:{"balance":hex(10**19)}, EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
               "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    for pool, market in directory["directory"].items():
        k=market["key"]
        key=(k["currency0"],k["currency1"],int(k["fee"],0),int(k["tickSpacing"]),k["hooks"])
        stable=next(t for t in key[:2] if t!=WETH)
        key_bytes=encode([KEY],[key])
        funding,=c.one(UNISWAP_V3,"getPool(address,address,uint24)",["address","address","uint24"],[WETH,stable,500],["address"])
        if c.one(funding,"factory()",returns=["address"])[0]!=UNISWAP_V3:
            raise ValueError("Funding pool not registered on independent Uniswap V3")
        def fund_sale(qty):
            eth=10**16
            return [tx(WETH,"deposit()",value=eth),tx(WETH,"approve(address,uint256)",["address","uint256"],[EXEC,eth]),
                    v5(WETH,stable,eth,qty,stamp+120,(1,funding,WETH,stable,10000,b""))]
        def native(x,y,qty,floor,end,proof=good):
            return tx(ADAPTER,"swap("+KEY+",bool,uint128,uint128,"+ATTEST+"[],address,uint256)",
                      [KEY,"bool","uint128","uint128",ATTEST+"[]","address","uint256"],[key,x==key[0],qty,floor,proof,OWNER,end])
        # A new block starts locked; a previous block's opening alone cannot finance the trade.
        q=10**12
        prefix=[tx(WETH,"deposit()",value=q),tx(WETH,"approve(address,uint256)",["address","uint256"],[ADAPTER,q])]
        plain=v5(WETH,stable,q,1,stamp+120,(19,MANAGER,WETH,stable,10000,key_bytes))
        checks=[native(WETH,stable,q,1,stamp+120,[(height,b"")]),native(WETH,stable,q,1,stamp+120,[(height-1,data)])]
        separate=prefix[:1]+[tx(WETH,"approve(address,uint256)",["address","uint256"],[EXEC,q]),plain]
        closed=simulate_block(url,at,overrides,prefix+checks)
        unassisted=simulate_block(url,at,overrides,separate)
        if any(x["status"]!="0x1" for x in closed[:2]+unassisted[:2]) or any(x["status"]!="0x0" for x in closed[2:]+unassisted[2:]):
            raise ValueError("Empty or wrong-block data did not have the expected boundary")
        unlocked=tx(HOOK,"unlockWithEmptyAttestation(address,bytes)",["address","bytes"],[node,data[20:]])
        opened=simulate_block(url,at,overrides,separate[:2]+[unlocked,plain])
        if opened[2]["status"]!="0x1":raise ValueError("Public attestation not accepted by deployed hook")
        controls.append(dict(pool=pool,empty=closed[2],wrong_block=closed[3],unassisted_v5=unassisted[2],
                             public_unlock=opened[2],v5_after_unlock=opened[3]))
        for x,sizes,fund in [(WETH,[10**12,10**14],None),(stable,[10000,1000000],fund_sale)]:
            for size in sizes:
                a=dict(pool=pool,input=x,size=str(size))
                payments.append(a)
                try:
                    legs,=direct_wallet(c,dict(tokens=key[:2],pool=pool),[size],stamp,0,ADAPTER,wrapped=WETH,
                                        fund_sale=fund,audit=True,keep_partial=True,modes=["router"],
                                        build_action=lambda x,y,n,minimum,end,mode:native(x,y,n,minimum,end))
                    if "sell_error" in legs and not legs["sell_error"].startswith("could not settle router:"):
                        raise ValueError(legs["sell_error"])
                    a.update(legs)
                except ValueError as error:
                    if not str(error).startswith("could not settle router:"):raise
                    a["reverted"]=str(error)[:260]
    if not all(any(a["pool"]==p and ("buy" in a or "sell" in a) for a in payments) for p in directory["directory"]):
        raise ValueError("Missing genuine taker payment on a registered market")
    return dict(chain=1,parent=at,block=height,canonical_hash=head["hash"],public_source=witness["url"],signer=node,
                source_data=message["data"],owner=OWNER,executor=EXEC,adapter=ADAPTER,controls=controls,payments=payments)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("output",type=Path)
    ap.add_argument("--endpoint")
    args=ap.parse_args()
    failures=[]
    for endpoint in ([args.endpoint] if args.endpoint else ["https://eth.drpc.org",GATEWAY,"https://rpc.mevblocker.io"]):
        try:
            result=record(endpoint)
            break
        except (requests.RequestException,RuntimeError,ValueError,KeyError) as error:
            failures.append([endpoint,str(error)[:350]])
    else:
        raise RuntimeError(failures)
    args.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
