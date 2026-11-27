"""Восстанавливает точные Bin PoolId по реальным квитанциям и событиям Initialize на Base."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from record_base_launch_bindings import read
from record_base_launch_hooks import DATA

RECEIPTS = (
    "0x6a84a00115f3601d9ff2a4c5e743c95de8cb2e3cc9937730d9bcffd5df26005c",
    "0x4e16154c9079f2abefdf99715ea7b087f0a5619c31414fbdf61412c4e7487fa4",
    "0xe2d1d13bc69473839502e9bfa707a8b70114f5ef0a1052e0b3d34e908d76c8bf",
)
KEY = ["address", "address", "address", "address", "uint24", "bytes32"]
INIT = "0x" + keccak(text="Initialize(bytes32,address,address,address,uint24,bytes32,uint24)").hex()
SWAP = "0x" + keccak(text="Swap(bytes32,address,int128,int128,uint24,uint24,uint16)").hex()

def record():
    b = json.loads((DATA/"omni_flux_roles.json").read_bytes())
    at, manager = b["selector"], b["roles"]["binPoolManager"]
    def call(sig, types, args, height):
        data = "0x" + (keccak(text=sig)[:4] + encode(types,args)).hex()
        return bytes.fromhex(read("eth_call",[{"to":manager,"data":data},height])[2:])
    def header(n): return read("eth_getBlockByNumber",[hex(n),False])
    roots, observations = {}, []
    for tx in RECEIPTS:
        receipt = read("eth_getTransactionReceipt",[tx])
        n = int(receipt["blockNumber"],16)
        if receipt["blockHash"] != header(n)["hash"]: raise ValueError("квитанция вне канона")
        for log in receipt["logs"]:
            if log["address"].lower()==manager and log["topics"][0]==SWAP:
                roots[log["topics"][1]] = min(n,roots.get(log["topics"][1],n))
                observations.append(log)
    ranges, markets = [], {}
    for pid, hi in roots.items():
        lo = 32873799
        while hi-lo>1:
            mid=(hi+lo)//2
            raw=call("getSlot0(bytes32)",["bytes32"],[bytes.fromhex(pid[2:])],hex(mid))
            if any(raw): hi=mid
            else: lo=mid
        lo,hi=hi-300,hi+699
        ranges.append([lo,hi])
        events=read("eth_getLogs",[dict(address=manager,fromBlock=hex(lo),toBlock=hex(hi),topics=[INIT])])
        if not any(l["topics"][1]==pid for l in events):raise ValueError("не найден первый Initialize")
        for log in events:
            id=log["topics"][1]; raw_id=bytes.fromhex(id[2:])
            if header(int(log["blockNumber"],16))["hash"]!=log["blockHash"]:raise ValueError("Initialize вне канона")
            raw=call("poolIdToPoolKey(bytes32)",["bytes32"],[raw_id],at);key=decode(KEY,raw)
            if keccak(raw)!=raw_id or key[3]!=manager:raise ValueError("ключ не соответствует рынку")
            h, fee, params, _=decode(["address","uint24","bytes32","uint24"],bytes.fromhex(log["data"][2:]))
            if (log["topics"][2][-40:],log["topics"][3][-40:],h,fee,params)!=(key[0][2:],key[1][2:],key[2],key[4],key[5]):raise ValueError("Initialize имеет другой ключ")
            slot=decode(["uint24"]*3,call("getSlot0(bytes32)",["bytes32"],[raw_id],at))
            amounts=decode(["uint128","uint128","uint256","uint256"],call("getBin(bytes32,uint24)",["bytes32","uint24"],[raw_id,slot[0]],at))
            markets[id]=dict(id=id,key=[*key[:-1],"0x"+key[-1].hex()],initialize=log,slot0=slot,active_bin=list(map(str,amounts)))
    return dict(selector=at,manager=manager,observed_swaps=observations,searched_blocks=ranges,markets=list(markets.values()))
if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path);a=p.parse_args()
    a.output.write_bytes(gzip.compress(json.dumps(record(),indent=2).encode(),mtime=0))
