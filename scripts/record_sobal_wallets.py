"""Replay independent Vault and ArbExecutor V5 wallet swaps for the registered Sobal pools on Base."""
import argparse
import collections
import itertools
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, rpc, simulate_block
from record_oasisswap_v5 import allowance, balance, scalar, tx, word
from record_sobal_directory import VAULT

WETH = "0x4200000000000000000000000000000000000006"
FUND = "(address,bool,address,bool)"
SWAPS = "(bytes32,uint256,uint256,uint256,bytes)[]"
SINGLE = "(bytes32,uint8,address,address,uint256,bytes)"
PRIMARY = "https://mainnet.base.org"


def edges(pool):
    return [(a, b) for (a, x), (b, y) in itertools.permutations(zip(pool["tokens"], map(int, pool["balances"])), 2)
            if x and y and pool["pool"] not in (a, b) and pool["families"]]


def funding_paths(markets, target, source):
    todo = collections.deque([([WETH], [])])
    while todo:
        tokens, pools = todo.popleft()
        if tokens[-1] == source:
            yield tokens, pools
            continue
        if len(pools) >= 3:
            continue
        for pool in markets:
            if pool["id"] == target:
                continue
            for a, b in edges(pool):
                if a == tokens[-1] and b not in tokens:
                    todo.append((tokens + [b], pools + [pool]))


def quote_call(pid, a, b, amount, wallet):
    return tx(VAULT, "queryBatchSwap(uint8," + SWAPS + ",address[]," + FUND + ")",
              ["uint8", SWAPS, "address[]", FUND],
              [0, [(pid, 0, 1, amount, b"")], [a, b], (wallet, False, OWNER, False)])


def approve(token, spender, amount):
    return tx(token, "approve(address,uint256)", ["address", "uint256"], [spender, amount])


def fund(tokens, pools, amount, deadline):
    budget = 10 ** 18
    if not pools:
        return [tx(WETH, "deposit()", value=amount)]
    steps = [(bytes.fromhex(p["id"][2:]), i, i + 1, amount if i == len(pools)-1 else 0, b"")
             for i, p in reversed(list(enumerate(pools)))]
    limits = [budget] + [0] * (len(tokens) - 2) + [-amount]
    trade = tx(VAULT, "batchSwap(uint8,"+SWAPS+",address[],"+FUND+",int256[],uint256)",
               ["uint8", SWAPS, "address[]", FUND, "int256[]", "uint256"],
               [1, steps, tokens, (OWNER, False, OWNER, False), limits, deadline])
    return [tx(WETH, "deposit()", value=budget), approve(WETH, VAULT, budget), trade]


def capture(directory, endpoint, shard):
    tag = directory["selector"]
    header = rpc(endpoint, "eth_getBlockByHash", [tag["blockHash"], False])
    if int(rpc(endpoint,"eth_chainId",[]),16) != 8453 or header["stateRoot"] != directory["state_root"]:
        raise ValueError("canonical Base archive mismatch")
    deadline = int(header["timestamp"], 16) + 200
    runtime = RUNTIME.read_text().strip().removeprefix("0x")
    env = {OWNER: {"balance": hex(2 * 10**18)}, EXEC: {"code": "0x" + runtime,
           "stateDiff": {word(0): word(int(OWNER, 16)), word(1): word(int(WETH, 16))}}}
    def run(calls):
        return simulate_block(endpoint,tag,env,calls)
    def quoted(result, amount):
        if result["status"] != "0x1":
            return None
        deltas, = decode(["int256[]"], bytes.fromhex(result["returnData"][2:]))
        if len(deltas) != 2 or deltas[0] != amount or deltas[1] > 0:
            raise ValueError("unexpected queryBatchSwap delta")
        return -deltas[1]

    markets = directory["pools"]
    rows = []
    for m in markets:
        if m["nonce"] % 4 != shard:
            continue
        pid = bytes.fromhex(m["id"][2:])
        for a, b in edges(m):
            reserve = int(m["balances"][m["tokens"].index(a)])
            for n in sorted({max(1,reserve//10000), max(1,reserve//1000)}):
                row = dict(pool=m["pool"], nonce=m["nonce"], family=m["families"][0], a=a, b=b, amount=str(n))
                rows.append(row)
                q = run([quote_call(pid,a,b,n,OWNER),quote_call(pid,a,b,n,EXEC)])
                amounts = [quoted(x,n) for x in q]
                row["quotes"] = [str(x) if x is not None else None for x in amounts]
                if any(x is None or x == 0 for x in amounts):
                    row["quote_errors"] = [x.get("error") for x in q]
                    continue
                errors = []
                for tokens, path in funding_paths(markets, m["id"], a):
                    prefix = fund(tokens,path,n,deadline)
                    # Funding may change a composable rate. Ask again after preparation.
                    res = run(prefix + [quote_call(pid,a,b,n,OWNER), quote_call(pid,a,b,n,EXEC)])
                    if all(x["status"] == "0x1" for x in res):
                        row["funding"] = [p["id"] for p in path]
                        break
                    errors.append(next(x.get("error") for x in res if x["status"] != "0x1"))
                else:
                    row["funding_failures"] = dict(attempts=len(errors), example=errors[:1])
                    continue
                for i, mode in enumerate(("vault","v5")):
                    settled_quote = quoted(res[len(prefix)+i], n)
                    if not settled_quote:
                        raise ValueError("positive quote lost after independent funding")
                    dest = (VAULT, EXEC)[i]
                    funds = (OWNER, False, OWNER, False)
                    def trade(floor):
                        if mode == "vault":
                            return tx(VAULT, "swap("+SINGLE+","+FUND+",uint256,uint256)", [SINGLE,FUND,"uint256","uint256"],
                                      [(pid,0,a,b,n,b""),funds,floor,deadline])
                        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                                  ["address","uint256","address","uint256","uint256",STEP+"[]"],
                                  [a,n,b,floor,deadline,[(17,VAULT,a,b,10000,encode(["bytes32"],[pid]))]])
                    reads = [balance(a,OWNER), balance(b,OWNER), balance(a,EXEC), balance(b,EXEC),
                             allowance(a,OWNER,dest), allowance(a,EXEC,VAULT)]
                    seq = prefix+[balance(a,VAULT)]+reads[4:]+[approve(a,dest,n)]+reads+[trade(settled_quote+1)]+reads+[trade(settled_quote)]+reads+[balance(a,VAULT)]
                    result = run(seq)
                    initial_grants=list(map(scalar,result[len(prefix)+1:len(prefix)+3]))
                    at=len(prefix)+4
                    before=list(map(scalar,result[at:at+6]))
                    over=result[at+6]
                    unchanged=list(map(scalar,result[at+7:at+13]))
                    fill=result[at+13]
                    after=list(map(scalar,result[at+14:-1]))
                    vault_credit=scalar(result[-1])-scalar(result[len(prefix)])
                    if any(x["status"]!="0x1" for x in result[:at]) or unchanged!=before or over["status"]!="0x0":
                        raise ValueError(f"{m['pool']} {mode}: rejected limit failed to restore wallet")
                    if fill["status"]!="0x1":
                        row[mode] = dict(rejected=fill.get("error"),minimum=str(settled_quote))
                        continue
                    persistent_vault = ((1<<256)-1)
                    for spender, start, end in zip((dest,VAULT),initial_grants,after[4:],strict=True):
                        # BalancerPoolToken grants its own Vault unlimited effective allowance intrinsically.
                        if end and not (spender==VAULT and start==end==persistent_vault):
                            raise ValueError((m["pool"],a,mode,"unexpected approval",start,end))
                    if (vault_credit,before[0]-after[0],after[1]-before[1],before[2:4],after[2:4]) != (n,n,settled_quote,[0,0],[0,0]):
                        raise ValueError((m["pool"],a,b,mode,n,settled_quote,"wallet",before,after))
                    row[mode]=dict(output=str(settled_quote),credited_to_vault=str(vault_credit),initial_allowances=list(map(str,initial_grants)),final_allowances=list(map(str,after[4:])),gas=int(fill["gasUsed"],16),rejected_plus_one=over.get("error"))
    if not any(r.get("v5",{}).get("output") and r.get("vault",{}).get("output") for r in rows):
        raise ValueError("no paid wallet control in this group")
    return dict(selector=tag, chain=8453, state_root=header["stateRoot"], executor="0x"+keccak(hexstr=runtime).hex(), rows=rows)


if __name__ == "__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("shard",type=int,choices=range(4))
    ap.add_argument("output",type=Path)
    args=ap.parse_args()
    d=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/sobal_base_directory.json").read_bytes())
    args.output.write_text(json.dumps(capture(d, PRIMARY,args.shard),indent=2)+"\n",encoding="utf8")
