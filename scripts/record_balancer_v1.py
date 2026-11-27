"""Capture the original Balancer V1 factory and measurements at a canonical Ethereum block."""
import argparse
import json
from collections import Counter
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT
from record_ethereum_v2_factories import STABLES, WETH
from record_balancer_v1_cash import payments

FACTORY = "0x9424b1412450d0f8fc2255faf6046b98213b76bd"
DOCS = "https://github.com/balancer/docs-v1/blob/5de0cee54dcdca2ae7e90c59bf615cfbcd630dd5/smart-contracts/addresses.md"
SOURCE = "https://github.com/balancer/balancer-core/tree/f4ed5d65362a8d6cec21662fb6eae233b0babc1f/contracts"
SWAP = "0x" + keccak(text="LOG_SWAP(address,address,address,uint256,uint256)").hex()


def created(factory, nonce):
    # RLP([address, nonce]) for a contract creator. BFactory's only CREATE is new BPool().
    raw = nonce.to_bytes((nonce.bit_length()+7)//8, "big")
    encoded = raw if 0 < nonce < 128 else bytes([128 + len(raw)]) + raw
    payload = bytes([148]) + bytes.fromhex(factory[2:]) + encoded
    return "0x" + keccak(bytes([192+len(payload)]) + payload)[-20:].hex()


def rpc(url, method, params):
    if method not in {"eth_chainId", "eth_getCode", "eth_getLogs", "eth_getBlockByNumber",
                      "eth_getTransactionCount", "eth_simulateV1"}:
        raise ValueError(method)
    response = requests.post(url,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=40)
    response.raise_for_status()
    value = response.json()
    if "error" in value:
        raise RuntimeError(str(value["error"])[:240])
    return value["result"]


def fingerprint(url, addr, at):
    raw = bytes.fromhex(rpc(url,"eth_getCode",[addr,at])[2:])
    if not raw:
        raise ValueError("missing bytecode: " + addr)
    return {"bytes":len(raw), "keccak256":"0x"+keccak(raw).hex()}


def record(endpoint, log_endpoint):
    chain = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    at, h = chain["selector"], chain["block"]
    c = Capture(dict(chain, endpoint=endpoint))
    for url in {endpoint, log_endpoint}:
        header = rpc(url, "eth_getBlockByNumber", [hex(h),False])
        if rpc(url, "eth_chainId", []) != "0x1" or header["hash"] != at["blockHash"]:
            raise ValueError("wrong chain or noncanonical block")
    nonce = int(rpc(endpoint,"eth_getTransactionCount",[FACTORY,at]),16)
    registered = [created(FACTORY,i) for i in range(1,nonce)]
    flags = posts(c,[(FACTORY,"isBPool(address)",["address"],[p]) for p in registered])
    if not registered or not all(decode(["bool"],flag)[0] for flag in flags):
        raise ValueError("factory creation history differs from isBPool")
    start = h - 7200
    logs = rpc(log_endpoint,"eth_getLogs",[dict(fromBlock=hex(start),toBlock=hex(h),topics=[SWAP])])
    if any(l.get("removed") or l["topics"][0] != SWAP for l in logs):
        raise ValueError("removed or foreign log")
    counts = Counter(l["address"].lower() for l in logs)
    markets = sorted(set(registered) & counts.keys(), key=lambda p:(-counts[p],p))
    queries = [(p,sig,[],[]) for p in markets for sig in
               ("getCurrentTokens()","getSwapFee()","isPublicSwap()","isFinalized()")]
    metadata = posts(c,queries)
    book = {}
    samples = []
    for idx,p in enumerate(markets):
        coins,fee,public,final = (decode([kind],v)[0] for kind,v in zip(
            ("address[]","uint256","bool","bool"),metadata[4*idx:4*idx+4],strict=True))
        book[p] = dict(events=counts[p],coins=list(coins),swap_fee=str(fee),public=public,finalized=final)
        stable = next((t for t in STABLES if t in coins),None)
        if stable and public and WETH in coins and len(samples) < 3:
            samples.append((p,stable))
    cases = []
    for p,stable in samples:
        m = book[p]
        m["code"] = fingerprint(endpoint,p,at)
        states = posts(c,[(p,sig,["address"],[t]) for t in m["coins"]
                          for sig in ("getBalance(address)","getDenormalizedWeight(address)")])
        m["assets"] = {t:{"balance":str(int.from_bytes(states[i*2],"big")),
                          "weight":str(int.from_bytes(states[i*2+1],"big"))} for i,t in enumerate(m["coins"])}
        cases.extend(payments(c,rpc,p,m,stable,int(header["timestamp"],16)))
    if not all(any(row.get("spent") and row["exact_input"] == exact and (row["source"] == WETH) == weth
                   for row in cases) for exact in (True,False) for weth in (True,False)):
        raise ValueError("missing paid original cash form")
    if rpc(log_endpoint,"eth_getBlockByNumber",[hex(h),False])["hash"] != at["blockHash"]:
        raise ValueError("end block changed")
    return dict(chain_id=1, block=h, selector=at, interval=[start,h], docs=DOCS, source=SOURCE,
                factory=FACTORY,factory_code=fingerprint(endpoint,FACTORY,at),next_nonce=nonce,registered=registered,
                all_swap_emitters=len(counts), original_events=sum(counts[p] for p in markets), markets=book, payments=cases)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out",type=Path)
    args = parser.parse_args()
    errors = []
    for url in ("https://rpc.mevblocker.io","https://eth.drpc.org","https://ethereum-rpc.publicnode.com"):
        try:
            data = record(url,"https://rpc.mevblocker.io")
            break
        except (requests.RequestException,RuntimeError,ValueError,KeyError) as exc:
            errors.append((url,str(exc)[:240]))
    else:
        raise RuntimeError(errors)
    args.out.write_bytes(json.dumps(data,separators=(",",":")).encode())
