"""Read every original BPool's token set and trading flags at the same Ethereum block as the factory proof."""
import argparse
import gzip
import json
import time

import requests
from pathlib import Path

from eth_abi import decode

from record_balancer_markets import Capture, posts
from record_balancer_v1 import created, rpc


def record(registry, endpoints):
    endpoint = endpoints[0]
    at = registry["selector"]
    factory = registry["factory"]
    chain = int(rpc(endpoint, "eth_chainId", []),16)
    if chain != registry["chain_id"] or not at["requireCanonical"]:
        raise ValueError("wrong chain or noncanonical selector")
    nonce = int(rpc(endpoint,"eth_getTransactionCount",[factory,at]),16)
    pools = [created(factory,i) for i in range(1,nonce)]
    if pools != registry["registered"]:
        raise ValueError("factory nonce differs from proven registry")
    readers = [Capture(dict(selector=at,endpoint=url)) for url in endpoints
               if int(rpc(url, "eth_chainId", []),16) == chain]
    rows=[]
    for off in range(0,len(pools),48):
        group=pools[off:off+48]
        calls=[(p,name,[],[]) for p in group for name in
               ("getCurrentTokens()","isPublicSwap()","isFinalized()")]
        errors=[]
        for attempt in range(6):
            c=readers[(off//48+attempt)%len(readers)]
            try:
                raw=posts(c,calls)
                break
            except (requests.RequestException,ValueError,RuntimeError) as exc:
                errors.append(str(exc)[:240])
                time.sleep(min(2**attempt,12))
        else:
            raise RuntimeError((off,errors))
        time.sleep(0.3)
        for index,pool in enumerate(group):
            tokens,public,finalized=(decode([typ],v)[0] for typ,v in zip(
                ("address[]","bool","bool"),raw[3*index:3*index+3],strict=True))
            if len(tokens)!=len(set(tokens)) or len(tokens)>8:
                raise ValueError(("invalid token set",pool,tokens))
            rows.append([list(tokens),public,finalized])
        c.calls.clear()
    if rpc(endpoint,"eth_getBlockByNumber",[hex(registry["block"]),False])["hash"] != at["blockHash"]:
        raise ValueError("block reorganized")
    return dict(chain=chain, block=registry["block"],selector=at,factory=factory,
                first_nonce=1,next_nonce=nonce,fields=["tokens","public_swap","finalized"],pools=rows)


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("registry",type=Path)
    parser.add_argument("output",type=Path)
    parser.add_argument("--rpc",required=True,action="append")
    args=parser.parse_args()
    data=record(json.loads(gzip.decompress(args.registry.read_bytes())),args.rpc)
    args.output.write_bytes(gzip.compress(json.dumps(data,separators=(",",":")).encode(),mtime=0))
