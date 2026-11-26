"""Replay canonical Base V2 factory registries and verify wallet payments against each deployed invariant."""
import argparse
import json
from pathlib import Path

import requests
from eth_utils import keccak

from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT,rpc
from record_ethereum_v2_factories import directory,digest,raw

MARKETS={
    "kewlswap-base":("0x5636a64b835f4e3821c798fda16e0ba106357646",1000,3,"KEWL: K",8127,8707),
    "pixelswap-base":("0xd07739a9e9c46d3deded97c0edc49cea8bab1bb9",1000,3,"SwapV2: K",1742,9133),
    "satori-base":("0x4858c605862a91a34d83c19a9704f837f64fa405",1000,3,"K",2171,11561),
    "candyswap":("0xbfd866bd2502e1693bc31735253a2fb9e728f714",10000,None,"CandySwap: K",2012,10282),
    "moonbase":("0xe396465a85dedb00fa8774162b106833de51ea41",10000,30,"UniswapV2: K",1760,9300),
    "x7-finance-base":("0x8b76c05676d205563ffc1cbd11c0a6e3d83929c5",100000,200,"Xchange: K",2738,14062),
    "9mm-v2-base":("0x4c1b8d4ae77a37b94e195cab316391d3c687ebd1",10000,25,"NineMM: K",2383,11632),
    "bakeryswap-base":("0xad376065d4fa9abb6dda0ded0eadd911fe6e0f92",1000,3,"SwapPair: K",2086,11745),
    "cbswap":("0x0a1c657b839a7fe9a976ffd4ac5a1a363de3ee69",1000,3,"CBSwap: K",1742,9124),
    "cloudbase":("0x0dccfdf227c3f7b1e5d34b2488a7b0ca01d400a4",1000,3,"UniswapV2: K",3596,15409),
    "icecreamswap-base":("0x9e6d21e759a7a288b80eef94e4737d313d31c13f",1000,3,"UniswapV2: K",1760,9203),
    "lfgswap-base":("0xe5552e0318531f9ec585c83bdc8956c08bf74b71",1000,3,"LfgSwap: K",2393,9087),
    "treble-v2":("0x6ae1d7efa0640b6a2fa393d1eff21fc38a08cd8f",1000,3,"Swap: K",1734,9075),
    "robots-farm-base":("0x77efb7acee1b0b4f26714c9a61c7de19c053e69f",10000,None,"UniswapV2: K",2243,9493),
}
WETH="0x4200000000000000000000000000000000000006"
USDC="0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
USDBC="0xd9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca"
ROUTER="0x4752ba5dbc23f44d87826276bf6fd6b1c372ad24"


def source(cap,row):
    factory,_,_,_,offset,size=row
    url="https://sourcify.dev/server/v2/contract/8453/"+factory
    response=requests.get(url,params={"fields":"compilation,sources,runtimeBytecode.onchainBytecode"},timeout=25)
    response.raise_for_status()
    src=response.json()
    code=raw(rpc(cap.endpoint,"eth_getCode",[factory,cap.selector]))
    if raw(src["runtimeBytecode"]["onchainBytecode"])!=code or offset+size>len(code):
        raise ValueError("verified deployment differs from historical runtime")
    invariant=[]
    for path,data in src["sources"].items():
        for line in data["content"].splitlines():
            if "balance0Adjusted =" in line or "balance1Adjusted =" in line or "getFeeForPair(token0, token1)" in line:
                invariant.append({"file":path,"line":line.strip()})
    if len(invariant)<2:raise ValueError("missing pair invariant in factory sources")
    return dict(url=url,runtime=digest(code),creation_code_hash=digest(code[offset:offset+size]),
                codecopy=[offset,size],invariant=invariant)


def standard_swap_origin(market,cap,row,proof):
    """Bind the ordinary swap fee to the exact pair creation bytes produced by its factory."""
    samples={"moonbase":"0xf429af2d7805bb1bed1ada3cacf2bbbed87294c8",
             "x7-finance-base":"0x9c38075ddee473e28a063cec3ae556c2ac0daa8f"}
    if market not in samples:return
    sample=samples[market]
    url="https://sourcify.dev/server/v2/contract/8453/"+sample
    result=requests.get(url,params={"fields":"runtimeBytecode.onchainBytecode,creationBytecode.onchainBytecode"},timeout=25)
    result.raise_for_status()
    pair=result.json()
    deployed=raw(rpc(cap.endpoint,"eth_getCode",[sample,cap.selector]))
    if (deployed!=raw(pair["runtimeBytecode"]["onchainBytecode"])
            or digest(raw(pair["creationBytecode"]["onchainBytecode"]))!=proof["creation_code_hash"]):
        raise ValueError("verified pair differs from canonical factory creation code")
    proof["verified_pair"]=url
    proof["standard_swap_fee"]={"entry":"swap(uint256,uint256,address,bytes)","numerator":row[2],"denominator":row[1]}


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("market",choices=MARKETS)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    row=MARKETS[args.market]
    for endpoint in ("https://mainnet.base.org","https://base.gateway.tenderly.co","https://base-rpc.publicnode.com"):
        try:
            cap=Capture(dict(snap,endpoint=endpoint))
            head=rpc(endpoint,"eth_getBlockByHash",[snap["selector"]["blockHash"],False])
            if head["hash"]!=snap["selector"]["blockHash"] or int(head["number"],16)!=snap["block"] or int(rpc(endpoint,"eth_chainId",[]),16)!=8453:
                raise ValueError("wrong chain or block")
            from base_v2_bytecode import PAIR_CODE, check_deployments, proof as bytecode_proof
            proof=bytecode_proof(args.market,cap,row) if args.market in PAIR_CODE else source(cap,row)
            standard_swap_origin(args.market,cap,row,proof)
            if row[2] is None:proof.setdefault("fee_getter","getFeeForPair(address,address)")
            created=None
            if args.market=="kewlswap-base":
                from record_kewlswap_base import create_directory, wallet
                proof["account_nonce"],created=create_directory(cap,row[0])
            result=directory(args.market,cap,created=created,config=row[:4],network="base",proof=proof,wrapped=WETH,stables=(USDC,USDBC),funding_router=ROUTER)
            if args.market in PAIR_CODE:check_deployments(cap,result)
            if args.market=="kewlswap-base":
                result["execution"]=wallet(cap,result,head)
            if args.market=="candyswap":
                from record_candyswap_base import capture
                result["net_payouts"]=capture(cap,result,head)
            if args.market=="bakeryswap-base":
                from record_bakeryswap_base import inspect
                result["native_router"]=inspect(cap,result,head)
            result.update(selector=cap.selector,height=snap["block"],state_root=head["stateRoot"],endpoint=endpoint)
            args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8")
            break
        except (ValueError,KeyError,requests.RequestException) as error:
            print(endpoint,str(error)[:150],flush=True)
    else:raise ValueError("no canonical archive returned the complete Base registry")
