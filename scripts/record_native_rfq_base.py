"""Native Base, публичная RFQ для собственного плательщика и CREATE в изолированном child.
Источники — Native V6 Address References, native_rfq_base_roles.json и штатная программа Ethereum.
"""
import argparse
import hashlib
import json
from functools import partial
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak

from record_native_rfq_ethereum import DATA, BASE, OWNER, leg, read, tx

WETH = "0x4200000000000000000000000000000000000006"
USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
CBBTC = "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf"
SWAP_ROUTER_02 = "0x2626664c2603336e57b271c5c0b26f421741e481"
node = partial(read, endpoints=("https://base.gateway.tenderly.co", "https://mainnet.base.org"))

def fund(src, amount, _end):
    if src == WETH:
        return tx(WETH, "deposit()", value=amount)
    # Покупка точного количества у штатного Uniswap Router02 за ETH данного адреса.
    typ = "(address,address,uint24,address,uint256,uint256,uint160)"
    cap = 10**17
    return tx(SWAP_ROUTER_02, "exactOutputSingle("+typ+")", [typ],
              [(WETH, src, 500, OWNER, amount, cap, 0)], cap)

def getter(address, sig, types, args, at):
    calldata = "0x" + (keccak(text=sig)[:4] + encode(types,args)).hex()
    return node("eth_call", [{"to":address,"data":calldata},at])

def checksum(raw):
    return "0x"+keccak(hexstr=raw).hex()

def capture(artifact):
    fixture = json.loads((DATA/"native_rfq_base_roles.json").read_bytes())
    roles = [fixture["roles"][k] for k in ("router","pool")]
    if int(node("eth_chainId",[]),16)!=fixture["chain_id"]:
        raise ValueError("другая сеть")
    header = node("eth_getBlockByNumber", ["latest",False])
    tag={"blockHash":header["hash"],"requireCanonical":True}
    roles_code = [node("eth_getCode",[r["address"],tag]) for r in roles]
    if [checksum(c) for c in roles_code]!=[r["runtime"] for r in roles]:
        raise ValueError("изменился официальный код Native")
    router,pool=[r["address"] for r in roles]
    engines=[]
    for token in (WETH,CBBTC):
        args=[token, USDC]
        pair=getter(pool,"getPairKey(address,address)",["address","address"],args,tag)
        engine=getter(pool,"getEngine(address,address)",["address","address"],args,tag)
        enabled=getter(pool,"directSwapEnabled(bytes32)",["bytes32"],[bytes.fromhex(pair[2:])],tag)
        engines.append(dict(tokens=args,key=pair,engine=engine,enabled=enabled))
    listed=getter(router,"isNativePools(address)",["address"],[pool],tag)
    constructor=encode(["address","bytes32","address","bytes32"],
        [router,bytes.fromhex(roles[0]["runtime"][2:]),pool,bytes.fromhex(roles[1]["runtime"][2:])])
    data="0x"+artifact["creation"]+constructor.hex()
    create={"from":OWNER,"data":data,"gas":hex(8_000_000)}
    deployed=node("eth_simulateV1",[{"blockStateCalls":[{"calls":[create]}]},tag])
    op=deployed[0]["calls"][0]
    if deployed[0]["parentHash"]!=header["hash"] or op["status"]!="0x1" or op["returnData"]=="0x":
        raise ValueError("не подтверждён CREATE")
    runtime=op["returnData"]
    sha=lambda raw:hashlib.sha256(bytes.fromhex(raw[2:])).hexdigest()
    card=dict(kind="native_rfq",chainId=8453,sourceBlock=header["hash"],data=data,
              creationSha256=sha(data),runtimeKeccak=checksum(runtime),runtimeSha256=sha(runtime),
              router=router,pool=pool,routerHash=roles[0]["runtime"],poolHash=roles[1]["runtime"])
    samples=[]
    for source,target,amounts in [(WETH,USDC,[10**15,10**16]),(USDC,WETH,[10**6,10**7]),
                                (CBBTC,USDC,[10**4,10**5]),(USDC,CBBTC,[10**6,10**7])]:
        for qty in amounts:
            for protected in (False,True):
                result=leg(roles,source,target,qty,artifact if protected else None,
                           slug="base",reader=node,funding=fund,strict_from_offer=True)
                samples.append(dict(mode="module" if protected else "router",**result))
    return dict(source=BASE,payer=OWNER,block=header["number"],block_hash=header["hash"],roles=roles,
                pamm=dict(registered=listed,pairs=engines),createGas=int(op["gasUsed"],16),card=card,
                runtime=runtime,observed=samples)

if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("output",type=Path)
    cli.add_argument("artifact",type=Path)
    args=cli.parse_args()
    args.output.write_text(json.dumps(capture(json.loads(args.artifact.read_bytes())),separators=(",",":")))
