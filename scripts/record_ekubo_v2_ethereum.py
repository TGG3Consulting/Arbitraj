"""Tie Ekubo Ethereum V2 activity to its own Core, pool registration and block-pinned state."""
import argparse
import concurrent.futures
import json
import time
from collections import Counter
from pathlib import Path

import requests
from eth_utils import keccak


ROOT = Path(__file__).resolve().parents[1]
CORE = "0xe0e0e08a6a4b9dc7bd67bcb7aade5cf48157d444"
ROUTER = "0x9995855c00494d039ab6792f18e368e530dff931"
DOCS = "https://docs.ekubo.org/reference/contracts/evm-v2/"
EVENT = "0x" + keccak(text="PoolInitialized(bytes32,(address,address,bytes32),int32,uint96)").hex()
ARCHIVE = "https://mainnet.gateway.tenderly.co"
LOGS = "https://ethereum.public.blockpi.network/v1/rpc/public"


def rpc(url, method, args):
    for delay in (1, 2, 4, 8, 16):
        result = requests.post(url, json={"jsonrpc":"2.0","id":1,"method":method,"params":args}, timeout=20)
        if result.status_code != 429:
            break
        time.sleep(max(delay, int(result.headers.get("Retry-After", 0))))
    result.raise_for_status()
    reply = result.json()
    if "error" in reply:
        raise RuntimeError(f"{method} {reply['error']}")
    return reply["result"]


def raw(hexstr):
    return bytes.fromhex(hexstr.removeprefix("0x"))


def passport(address, block):
    url = f"https://eth.blockscout.com/api/v2/smart-contracts/{address}"
    r = requests.get(url, timeout=24)
    r.raise_for_status()
    meta = r.json()
    code = raw(rpc(ARCHIVE, "eth_getCode", [address, block]))
    if not code or not meta["is_fully_verified"] or code != raw(meta["deployed_bytecode"]):
        raise ValueError(f"unverified runtime {address}")
    return {"address":address, "runtime":"0x"+keccak(code).hex(), "bytes":len(code), "source":url,
            "compiler":meta["compiler_version"]}, meta


def signed(data):
    return int.from_bytes(data, "big", signed=True)


def swap(event):
    d = raw(event["data"])
    if event["topics"]:
        return None
    if len(d) != 116:
        raise ValueError("unknown anonymous Core log")
    return {"block":int(event["blockNumber"], 16), "block_hash":event["blockHash"],
            "tx":event["transactionHash"], "index":int(event["logIndex"], 16), "wire":event["data"],
            "locker":"0x"+d[:20].hex(), "id":"0x"+d[20:52].hex(),
            "delta0":str(signed(d[52:68])), "delta1":str(signed(d[68:84]))}


def market(pid, height, tag, sample):
    slot = "0x" + keccak(raw(pid)+(2).to_bytes(32, "big")).hex()
    state = rpc(ARCHIVE, "eth_getStorageAt", [CORE, slot, tag])
    if not int(state,16):
        raise ValueError("active pool missing")
    lo, hi = 0, height
    while hi - lo > 1:
        mid = (hi+lo)//2
        if int(rpc(ARCHIVE, "eth_getStorageAt", [CORE,slot,hex(mid)]),16):
            hi = mid
        else:
            lo = mid
    header = rpc(ARCHIVE, "eth_getBlockByNumber", [hex(hi),False])
    events = rpc(ARCHIVE, "eth_getLogs", [{"address":CORE,"blockHash":header["hash"],"topics":[EVENT]}])
    event, = [e for e in events if e["data"].startswith(pid) and not e["removed"]]
    data = raw(event["data"])
    if len(data)!=192 or keccak(data[32:128])!=raw(pid) or event["blockHash"]!=header["hash"]:
        raise ValueError("PoolInitialized key does not hash to traded ID")
    key = {"token0":"0x"+data[44:64].hex(), "token1":"0x"+data[76:96].hex(),
           "config":"0x"+data[96:128].hex(), "extension":"0x"+data[96:116].hex(),
           "fee_x64":str(int.from_bytes(data[116:124])), "tick_spacing":int.from_bytes(data[124:128])}
    quotes=[]
    for direction in (0,1):
        observed = sorted(abs(int(s["delta"+str(direction)])) for s in sample
                          if int(s["delta"+str(direction)])>0)
        if not observed:
            opposite = sorted(abs(int(s["delta"+str(direction)])) for s in sample
                              if int(s["delta"+str(direction)])<0)
            if not opposite:
                continue
            observed=opposite
        for size in sorted({max(1, observed[len(observed)//2]//4), observed[len(observed)//2]}):
            args=data[32:128]+b"".join(n.to_bytes(32,"big") for n in (direction,size,79227682466138141934206691491 if direction else 4611797791050542631,0))
            wire="0x"+(keccak(text="quote((address,address,bytes32),bool,int128,uint96,uint256)")[:4]+args).hex()
            try:
                out = raw(rpc(ARCHIVE, "eth_call", [{"to":ROUTER,"data":wire},tag]))
            except RuntimeError as err:
                quotes.append(dict(direction=direction, amount=str(size),error=str(err)[:250]))
                continue
            if len(out)!=64:
                raise ValueError("unexpected V2 router quote")
            d0,d1=signed(out[:32]),signed(out[32:])
            quotes.append(dict(direction=direction, amount=str(size),delta0=str(d0),delta1=str(d1),
                               full=(d1 if direction else d0)==size and (d0 if direction else d1)<0))
    return dict(id=pid, key=key, slot=slot, state=state, liquidity=str(int(state,16)>>128),
                registration=event, quotes=quotes, swaps=len(sample))


def collect():
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    h,tag=snap["block"],snap["selector"]
    start=h-7199
    def header(n):
        return rpc(ARCHIVE,"eth_getBlockByNumber",[hex(n),False])
    a,b=header(start),header(h)
    if int(rpc(ARCHIVE,"eth_chainId",[]),16)!=1 or tag["blockHash"]!=b["hash"]:
        raise ValueError("wrong chain or anchor")
    core,source=passport(CORE,tag)
    router,rt=passport(ROUTER,tag)
    if "log0(o, 116)" not in source["source_code"] or "mstore(32, 2)" not in next(
        v["source_code"] for v in rt["additional_sources"] if v["file_path"].endswith("CoreLib.sol")):
        raise ValueError("unexpected log or storage layout")
    events=[]
    if int(rpc(LOGS,"eth_chainId",[]),16)!=1:
        raise ValueError("wrong log chain")
    for n, expected in [(start,a),(h,b)]:
        if rpc(LOGS,"eth_getBlockByNumber",[hex(n),False])["hash"]!=expected["hash"]:
            raise ValueError("log service on another chain")
    for lo in range(start,h+1,3600):
        events+=rpc(LOGS, "eth_getLogs", [{"address":CORE,"fromBlock":hex(lo),"toBlock":hex(min(lo+3599,h))}])
    if any(e["removed"] or e["address"].lower()!=CORE or not start<=int(e["blockNumber"],16)<=h for e in events):
        raise ValueError("noncanonical or foreign log")
    if len({(e["blockHash"],e["logIndex"]) for e in events})!=len(events):
        raise ValueError("duplicate logs")
    swaps=[s for e in events if (s:=swap(e))]
    ids=sorted({s["id"] for s in swaps})
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        markets=list(pool.map(lambda pid:market(pid,h,tag,[s for s in swaps if s["id"]==pid]),ids))
    if header(h)["hash"]!=tag["blockHash"] or rpc(LOGS,"eth_getBlockByNumber",[hex(h),False])["hash"]!=b["hash"]:
        raise ValueError("log window reorganized")
    return dict(docs=DOCS,chain=1,core=core,router=router,selector=tag,state_root=b["stateRoot"],
                window=[dict(number=int(x["number"],16),hash=x["hash"],time=int(x["timestamp"],16)) for x in (a,b)],
                logs=len(events),topics=dict(Counter(e["topics"][0] for e in events if e["topics"])),
                swaps=swaps, markets=markets)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    args.output.write_text(json.dumps(collect(),indent=2)+"\n",encoding="utf8")
