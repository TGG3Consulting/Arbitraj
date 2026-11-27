"""Сверяет реестр Angstrom L1 с кодом хранилища, наблюдает фактические V4 Swap и канонические платежи."""
import argparse
import gzip
import json
from collections import Counter
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import ROOT

HOOK = "0x0000000aa232009084bd71a5797d089aa4edfad4"
MANAGER = "0x000000000004444c5dc75cb358380d2e3de08a90"
INSPECTOR = "0xd262c224402e9a7f5e8ad1621be1bb1f3203b6c3"
ADAPTER = "0xb535aeb27335b91e1b5bccbd64888ba7574efbf8"
DATA = "https://mainnet.node.data.angstrom.xyz"
GATEWAY = "https://mainnet.gateway.tenderly.co"
EVENT = "0x" + keccak(text="Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)").hex()
INIT = "0x" + keccak(text="Initialize(bytes32,address,address,uint24,int24,address,uint160,int24)").hex()
DEPLOYMENT = 22971782


def rpc(endpoint, method, params):
    response = requests.post(endpoint, json=dict(jsonrpc="2.0", id=1, method=method, params=params), timeout=35)
    response.raise_for_status()
    payload = response.json()
    if "result" not in payload:
        raise ValueError((method, payload.get("error")))
    return payload["result"]


def record(endpoint):
    anchor = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    height, at = anchor["block"], anchor["selector"]
    query = lambda method, args: rpc(endpoint, method, args)
    header = lambda n: query("eth_getBlockByNumber", [hex(n), False])
    end = header(height)
    if end["hash"] != at["blockHash"] or query("eth_chainId", []) != "0x1":
        raise ValueError("Wrong Ethereum state")
    def call(to, sig, returns, args=(), values=()):
        calldata = keccak(text=sig)[:4] + encode(args, values)
        data = query("eth_call", [dict(to=to, data="0x" + calldata.hex()), at])
        return decode(returns, bytes.fromhex(data[2:]))
    def code(addr):
        result = query("eth_getCode", [addr, at])
        if result == "0x":
            raise ValueError(("No runtime", addr))
        return bytes.fromhex(result[2:])
    def logs(lo, topic, ids):
        return query("eth_getLogs", [dict(address=MANAGER, fromBlock=hex(lo), toBlock=hex(height), topics=[topic, ids])])

    if (call(INSPECTOR, "angstrom()", ["address"])[0] != HOOK or
        call(INSPECTOR, "uniswapPoolManager()", ["address"])[0] != MANAGER or
        call(ADAPTER, "poolManager()", ["address"])[0] != MANAGER):
        raise ValueError("Hook/inspector/adapter links disagree")
    store, = call(INSPECTOR, "configStore()", ["address"])
    program = code(store)
    if program[0] or (len(program) - 1) % 32:
        raise ValueError("Wrong ConfigStore bytecode layout")
    entries = {program[i:i+27]: (int.from_bytes(program[i+27:i+29], "big"), int.from_bytes(program[i+29:i+32], "big"))
               for i in range(1, len(program), 32)}
    pools = {}
    for row in rpc(DATA, "angstrom_allPoolKeys", [height]):
        k = row["pool_key"]
        a, b, fee, step = k["currency0"], k["currency1"], int(k["fee"], 0), int(k["tickSpacing"])
        key = keccak(encode(["address", "address"], [a, b]))[5:]
        if a >= b or k["hooks"] != HOOK or fee != 1 << 23 or entries.pop(key) != (step, int(row["pool_fee_in_e6"], 0)):
            raise ValueError("Data API differs from code at canonical ConfigStore")
        pid = "0x" + keccak(encode(["address","address","uint24","int24","address"],[a,b,fee,step,HOOK])).hex()
        lp, protocol = call(INSPECTOR, "unlockedFee(bytes27)", ["uint24","uint24"], ["bytes27"], [key])
        pools[pid] = dict(key=k, store_key="0x"+key.hex(), batch_fee=int(row["pool_fee_in_e6"], 0),
                          unlocked_lp=lp, unlocked_protocol=protocol)
    if entries or not pools:
        raise ValueError("Incomplete canonical market directory")
    births = logs(DEPLOYMENT, INIT, list(pools))
    if len(births) != len(pools) or {v["topics"][1] for v in births} != set(pools):
        raise ValueError("Physical market initialization mismatch")
    # Derive a 24-hour window by canonical timestamps, allowing missed Ethereum slots.
    lo, hi = height - 8000, height
    cutoff = int(end["timestamp"],16) - 86400
    if int(header(lo)["timestamp"],16) >= cutoff:
        raise ValueError("Insufficient historical interval")
    while lo < hi:
        mid = (lo + hi)//2
        if int(header(mid)["timestamp"],16) < cutoff:
            lo = mid + 1
        else:
            hi = mid
    swaps = logs(lo, EVENT, list(pools))
    groups, counts, totals = {}, Counter(), Counter()
    for row in swaps:
        if row.get("removed") or row["address"].lower()!=MANAGER or row["topics"][1] not in pools:
            raise ValueError("Foreign or removed Swap")
        pid, actor = row["topics"][1], "0x" + row["topics"][2][-40:]
        amount0, amount1, _, _, _, lp_fee = decode(["int128","int128","uint160","uint128","int24","uint24"],bytes.fromhex(row["data"][2:]))
        if amount0 == 0 or amount1 == 0:
            continue
        form = "bundle" if actor==HOOK else "adapter" if actor==ADAPTER else "other"
        counts[(pid, form, lp_fee)] += 1
        totals[pid] += abs(amount0)
        groups[(pid, form, amount0<0)] = row
    receipts = []
    for (pid, form, sell0), row in sorted(groups.items()):
        txh = row["transactionHash"]
        tx = query("eth_getTransactionByHash", [txh])
        receipt = query("eth_getTransactionReceipt", [txh])
        block = header(int(tx["blockNumber"],16))
        if (receipt["status"]!="0x1" or tx["blockHash"]!=receipt["blockHash"] or block["hash"]!=row["blockHash"]
                or tx["blockHash"]!=block["hash"] or block["transactions"][int(tx["transactionIndex"],16)] != txh):
            raise ValueError("Noncanonical receipt")
        if not any(l["logIndex"]==row["logIndex"] and l["topics"]==row["topics"] and l["data"]==row["data"] for l in receipt["logs"]):
            raise ValueError("Swap missing from receipt")
        receipts.append(dict(pool=pid, form=form, sell0=sell0, tx=tx, receipt=receipt,
                             header={k:block[k] for k in ["number","hash","parentHash","stateRoot","timestamp"]}))
    return dict(chain_id=1, block=height, selector=at, state_root=end["stateRoot"], store=store, store_runtime="0x"+program.hex(),
                runtime_hashes={a:"0x"+keccak(code(a)).hex() for a in [MANAGER,HOOK,ADAPTER,INSPECTOR]},
                controller=call(INSPECTOR,"controller()",["address"])[0],last_updated=call(INSPECTOR,"lastBlockUpdated()",["uint64"])[0],
                directory=pools, initialized=births, window={k:header(lo)[k] for k in ["number","hash","timestamp"]},
                window_end_utc_timestamp=int(end["timestamp"],16),total_swaps=len(swaps),
                nonzero=[dict(pool=p,actor=f,lp_fee=fee,count=n) for (p,f,fee),n in sorted(counts.items())],
                token0_volume={p:str(n) for p,n in totals.items()}, samples=receipts)


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    parser.add_argument("--endpoint",default=GATEWAY)
    args=parser.parse_args()
    result=json.dumps(record(args.endpoint),separators=(",",":")).encode()
    args.output.write_bytes(gzip.compress(result,mtime=0) if args.output.suffix==".gz" else result)
