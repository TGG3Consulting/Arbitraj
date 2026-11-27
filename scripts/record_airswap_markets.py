"""AirSwap V5: registry membership, code and fee terms on the same canonical block.
Official deployments https://github.com/airswap/airswap-about/blob/6363ffe1c1efd06d35f11fe1832c5e504341faad/technology/deployments.md
RFC RequestForQuoteERC20 https://github.com/airswap/airswap-about/blob/6363ffe1c1efd06d35f11fe1832c5e504341faad/technology/protocols.md
"""
import argparse
import hashlib
import json
from pathlib import Path
from urllib.parse import urlsplit
import requests
from eth_utils import keccak
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, rpc

REGISTRY = "0xe30e9c001defb5f0b04fd21662454a2427f4257a"
SWAP = "0xd82e10b9a4107939e55fcca9b53a9ede6cf2fc46"
LEGACY = "0xd82fa167727a4dc6d6f55830a2c47abbb4b3a0f8"
PROTOCOL = bytes.fromhex("02ad05d3")
ENDPOINTS = {
    "ethereum":["https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com"],
    "base":["https://mainnet.base.org","https://base-rpc.publicnode.com"],
}


def acquire(network, url):
    chain = json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())[network]
    at = {"blockHash":chain["hash"],"requireCanonical":True}
    header = rpc(url,"eth_getBlockByHash",[chain["hash"],False])
    assert header["hash"]==chain["hash"] and int(header["number"],16)==chain["block"]
    assert int(rpc(url,"eth_chainId",[]),16)==chain["chain"]
    c=Capture({"selector":at,"endpoint":url})
    def getter(contract,name,ret,args=(),vals=()):
        return c.one(contract,name,args,vals,[ret])[0]
    def runtime(a):
        code=bytes.fromhex(rpc(url,"eth_getCode",[a,at])[2:])
        return {"address":a,"bytes":len(code),"hash":"0x"+keccak(code).hex()} if code else None
    roles={"registry":runtime(REGISTRY),"swap":runtime(SWAP),"legacy":runtime(LEGACY)}
    assert roles["registry"] and roles["swap"]
    makers=getter(REGISTRY,"getStakersForProtocol(bytes4)","address[]",["bytes4"],[PROTOCOL])
    urls=getter(REGISTRY,"getServerURLsForStakers(address[])","string[]",["address[]"],[list(makers)]) if makers else []
    assert len(urls)==len(makers) and len(set(makers))==len(makers)
    result=[]
    for staker,locator in zip(makers,urls):
        tokens=getter(REGISTRY,"getTokensForStaker(address)","address[]",["address"],[staker])
        # Registry entries are untrusted. No network call is made to a discovered URL.
        result.append(dict(staker=staker,tokens=tokens,locator_sha256=hashlib.sha256(locator.encode()).hexdigest(),
                           https_url=urlsplit(locator).scheme=="https"))
    cash={}
    for generation,target in [("v5",SWAP),("v4",LEGACY)]:
        if generation=="v4" and not roles["legacy"]:
            continue
        cash[generation]={"fee":getter(target,"protocolFee()","uint256"),
                          "fee_light":getter(target,"protocolFeeLight()","uint256"),
                          "fee_wallet":getter(target,"protocolFeeWallet()","address")}
    return dict(chain_id=chain["chain"],height=chain["block"],selector=at,roles=roles,
                protocol="0x"+PROTOCOL.hex(),terms=cash,makers=result)

if __name__=="__main__":
    arg=argparse.ArgumentParser()
    arg.add_argument("network",choices=ENDPOINTS);arg.add_argument("output",type=Path)
    options=arg.parse_args()
    for url in ENDPOINTS[options.network]:
        try:
            snapshot=acquire(options.network,url)
            break
        except (ValueError,requests.RequestException,RuntimeError) as error:
            last=error
    else:
        raise RuntimeError("AirSwap canonical roles and makers could not be read") from last
    options.output.write_text(json.dumps(snapshot,separators=(",",":")),encoding="utf8")
