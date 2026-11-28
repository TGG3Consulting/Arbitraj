"""Record current/previous 0x taker Settlers on the first Base deployment and project snapshot."""
import argparse
import json
from pathlib import Path

import requests
from eth_utils import keccak

REGISTRY = "0x00000000000004533fe15556b1e086bb1a72ceae"
FIRST = 14769149
PROJECT = 62262756
URLS = ("https://mainnet.base.org", "https://base.gateway.tenderly.co", "https://base.api.pocket.network")


def request(method, params, allow_revert=False):
    packet = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
    for url in URLS:
        try:
            raw = requests.post(url, json=packet, timeout=8).json()
        except (requests.RequestException, ValueError):
            continue
        error = raw.get("error", {})
        if raw.get("result") is not None or allow_revert and error.get("code") in (3,-32000) and "revert" in error.get("message","").lower():
            return {"source":url, "request":packet, "response":raw}
    raise RuntimeError(f"{method} unavailable")


def role(signature, selector, allow_revert=False):
    data = "0x" + (keccak(text=signature)[:4] + (2).to_bytes(32,"big")).hex()
    return request("eth_call", [{"to":REGISTRY,"data":data},selector],allow_revert)


def capture():
    result = []
    for n in [FIRST, PROJECT]:
        block = request("eth_getBlockByNumber",[hex(n),False])["response"]["result"]
        selector = {"blockHash":block["hash"],"requireCanonical":True}
        current = role("ownerOf(uint256)", selector)
        previous = role("prev(uint128)", selector, n==FIRST)
        owner = "0x"+current["response"]["result"][-40:]
        if n == FIRST:
            before = request("eth_getCode",[owner,hex(n-1)])["response"]["result"]
            if before!="0x" or "error" not in previous["response"]:
                raise RuntimeError("First taker Settler control changed")
        code = bytes.fromhex(request("eth_getCode",[owner,selector])["response"]["result"][2:])
        if not code:
            raise RuntimeError("Missing taker Settler")
        result.append({"block":n, "selector":selector,"current":current,"previous":previous,
                       "settlerCodeHash":"0x"+keccak(code).hex()})
    return {"chainId":8453,"registry":REGISTRY,"feature":2,"observations":result}


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("output",type=Path)
    args=parser.parse_args();args.output.write_text(json.dumps(capture(),indent=2)+"\n",encoding="utf8")
