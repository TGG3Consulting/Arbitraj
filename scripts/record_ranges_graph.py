"""Archive a protected three-leg V5 swap and its single-leg alternative on the same Ethereum state."""
import argparse
import gzip
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from record_ranges_wallets import (
    AT, DATA, DYNAMIC, ENDPOINT, EXEC, OWNER, ROOT_HASH, RUNTIME, SIZES, STEP, WETH,
    allow, balance, buy_cash, read_batch, simulate_block, tx,
)


def snapshot(reply):
    if reply["status"] != "0x1":
        raise ValueError(("state read", reply.get("error")))
    raw, live, virtual, *_ = decode([DYNAMIC], bytes.fromhex(reply["returnData"][2:]))[0]
    return dict(raw=raw, live=live, virtual=virtual)


def compare(endpoint, shard):
    directory = json.loads(gzip.decompress(DATA.read_bytes()))
    roles = directory["roles"]
    chain, block, owner_code, exec_code = read_batch(endpoint, [
        ("eth_chainId", []), ("eth_getBlockByHash", [AT["blockHash"], False]),
        ("eth_getCode", [OWNER, AT]), ("eth_getCode", [EXEC, AT]),
    ])
    if (chain, block["stateRoot"], owner_code, exec_code) != ("0x1", ROOT_HASH, "0x", "0x"):
        raise ValueError("unexpected canonical state")
    deadline = int(block["timestamp"], 16) + 900
    word = lambda x: "0x" + format(x, "064x")
    overrides = {OWNER: {"balance": hex(10**18)}, EXEC: {
        "code": "0x" + RUNTIME.read_text().strip().removeprefix("0x"),
        "stateDiff": {word(0): word(int(OWNER, 16)), word(1): word(int(WETH, 16))}}}
    fixtures = []
    for i, pool in enumerate(directory["pools"]):
        if i % 4 != shard or not pool["initialized"] or pool["paused"] or pool["recovery"] or pool["hook_stopped"]:
            continue
        tokens = [t for t, q in zip(pool["tokens"], pool["raw"], strict=True) if q]
        cash = next((t for t in SIZES if t in tokens), None)
        if cash is None:
            continue
        targets = [t for t in tokens if t != cash]
        # Include both orderings; the middle token gets spent by the executor.
        for idx, mid in enumerate(targets):
            if len(targets) > 1:
                fixtures.append((pool["pool"], cash, mid, targets[(idx + 1) % len(targets)]))

    swap_topic = "0x" + keccak(text="Swap(address,address,address,uint256,uint256,uint256,uint256)").hex()

    def one(fixture):
        pool, cash, mid, dest = fixture
        qty = SIZES[cash][-1]
        prefix = [buy_cash(cash, qty, deadline), *allow("v5", roles, cash, qty, deadline)]
        checks = [balance(t, a) for a in (OWNER, EXEC) for t in (cash, mid, dest)]
        state = tx(pool, "getRangePoolDynamicData()")
        def attempt(hops, minimum, end):
            steps = [(23, roles["vault"], a, b, share, encode(["address"], [pool])) for a, b, share in hops]
            op = tx(EXEC, "execute(address,uint256,address,uint256,uint256," + STEP + "[])",
                    ["address","uint256","address","uint256","uint256",STEP+"[]"],
                    [cash,qty,dest,minimum,end,steps])
            replies = simulate_block(endpoint, AT, overrides, [*prefix, state, *checks, op, *checks, state])
            if any(r["status"] != "0x1" for r in replies[:len(prefix)]):
                raise ValueError(("funding", fixture, [r.get("error") for r in replies[:len(prefix)]]))
            before, *results, after = replies[len(prefix):]
            leg = results[len(checks)]
            accounts = results[:len(checks)] + results[len(checks)+1:]
            if any(r["status"] != "0x1" or len(r["returnData"]) != 66 for r in accounts):
                raise ValueError(("balance read", fixture))
            b, a = ([int(r["returnData"], 16) for r in group] for group in (accounts[:6], accounts[6:]))
            states = [snapshot(before), snapshot(after)]
            if leg["status"] != "0x1":
                if a != b or states[0] != states[1]:
                    raise ValueError(("revert retained state", fixture))
                return dict(stage="reverted",error=leg.get("error"))
            swaps = [log for log in leg["logs"] if log["address"].lower()==roles["vault"] and log["topics"][0].lower()==swap_topic]
            if len(swaps) != len(hops):
                raise ValueError(("swap count", fixture))
            amounts = []
            for log, (source,target,share) in zip(swaps,hops,strict=True):
                if [t.lower()[-40:] for t in log["topics"][1:]] != [x[2:] for x in (pool,source,target)]:
                    raise ValueError(("swap identity",fixture))
                paid, received, _, _ = decode(["uint256"]*4, bytes.fromhex(log["data"][2:]))
                amounts.append([source,target,share,paid,received])
            credit = a[2]-b[2]
            if b[0]-a[0] != qty or a[1] != b[1] or a[3:] != b[3:] or credit <= 0 or credit != int(leg["returnData"], 16):
                raise ValueError(("wallet accounting", fixture,b,a))
            return dict(stage="paid",credit=credit,gas=int(leg["gasUsed"],16),swaps=amounts,state=states)

        row = dict(pool=pool,cash=cash,mid=mid,dest=dest,size=qty)
        for name, hops in [("direct",[(cash,dest,10000)]),("split",[(cash,mid,4000),(mid,dest,10000),(cash,dest,10000)])]:
            result = attempt(hops,1,deadline)
            if result["stage"] == "paid":
                c = result["credit"]
                exact = attempt(hops,c,deadline)
                if exact["stage"] != "paid" or exact["swaps"] != result["swaps"] or exact["state"] != result["state"]:
                    raise ValueError(("exact minimum",fixture,name))
                if any(attempt(hops,m,e)["stage"] != "reverted" for m,e in ((c+1,deadline),(1,0))):
                    raise ValueError(("minimum or deadline", fixture,name))
            row[name]=result
        return row
    with ThreadPoolExecutor(max_workers=3) as workers:
        rows=list(workers.map(one,fixtures))
    return dict(selector=AT,state_root=ROOT_HASH,shard=shard,payer=OWNER,cases=rows)


if __name__ == "__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("path",type=Path)
    cli.add_argument("shard",type=int,choices=range(4))
    args=cli.parse_args()
    for endpoint in (ENDPOINT,"https://ethereum-rpc.publicnode.com","https://mainnet.gateway.tenderly.co"):
        try:
            evidence=compare(endpoint,args.shard)
            break
        except (OSError,RuntimeError) as exc:
            print(endpoint,str(exc)[:180])
    else:
        raise RuntimeError("no complete archive response")
    args.path.write_text(json.dumps(evidence,separators=(",",":")),encoding="utf8")
