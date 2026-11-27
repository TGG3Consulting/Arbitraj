"""Compare signed Balancer V3 Router and executor paths against real Puzzle Ranges wallet balances."""
import argparse
import gzip
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak

from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, rpc, simulate_block
from record_bitswap_ethereum import balance, read_batch, tx
from record_ethereum_v2_factories import FUNDING, WETH
from record_ranges_ethereum import AT, ENDPOINT, ROOT_HASH

DATA = ROOT / "tests/data/first_run_dex_2026-11-26/ranges_ethereum_directory.json.gz"
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
USDC = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
SIZES = {WETH: (10**13, 10**15), USDT: (100_000, 1_000_000), USDC: (100_000, 1_000_000)}
ZERO = "0x" + "00"*20
ENABLE = "approve(address,uint256)"


def packet(mode, roles, pool, source, dest, qty, end, minimum):
    if mode == "v5":
        step = (23, roles["vault"], source, dest, 10000, encode(["address"], [pool]))
        return tx(EXEC, "execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address","uint256","address","uint256","uint256",STEP+"[]"],
                  [source,qty,dest,minimum,end,[step]])
    return tx(roles["router"], "swapSingleTokenExactIn(address,address,address,uint256,uint256,uint256,bool,bytes)",
              ["address","address","address","uint256","uint256","uint256","bool","bytes"],
              [pool, source, dest, qty, minimum, end, False, b""])


def allow(mode, roles, token, qty, end):
    spender = EXEC if mode == "v5" else roles["permit2"]
    calls = [tx(token, ENABLE, ["address","uint256"], [spender,qty])]
    if mode == "router":
        calls.append(tx(spender, "approve(address,address,uint160,uint48)",
                        ["address","address","uint160","uint48"], [token,roles["router"],qty,end]))
    return calls


def buy_cash(cash, qty, end):
    if cash == WETH:
        return tx(WETH, "deposit()", value=qty)
    return tx(FUNDING, "swapETHForExactTokens(uint256,address[],address,uint256)",
              ["uint256","address[]","address","uint256"], [qty,[WETH,cash],OWNER,end], 10**17)


def run(endpoint, number):
    directory = json.loads(gzip.decompress(DATA.read_bytes()))
    roles = directory["roles"]
    chain, head, *codes = read_batch(endpoint,[("eth_chainId",[]),("eth_getBlockByHash",[AT["blockHash"],False]),
        *(("eth_getCode",[a,AT]) for a in [OWNER,EXEC,*roles.values()])])
    if (chain != "0x1" or head["stateRoot"] != ROOT_HASH or codes[:2] != ["0x","0x"]
            or any("0x"+keccak(hexstr=code).hex()!=directory["runtime"][k] for k,code in zip(roles,codes[2:],strict=True))):
        raise ValueError("unexpected wallet, role code or canonical state")
    deadline = int(head["timestamp"],16) + 900
    word = lambda n: "0x"+format(n,"064x")
    env = {OWNER:{"balance":hex(10**18)},EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
           "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    def simulate(calls):
        return simulate_block(endpoint,AT,env,calls)
    def verify(mode, pool, source, target, qty, prefix):
        checks = [balance(source,OWNER),balance(target,OWNER),balance(source,EXEC),balance(target,EXEC)]
        approval = allow(mode,roles,source,qty,deadline)
        probe=tx(roles["router"],"querySwapSingleTokenExactIn(address,address,address,uint256,address,bytes)",
                 ["address","address","address","uint256","address","bytes"],[pool,source,target,qty,OWNER,b""])
        probe["from"]=ZERO
        # A successful Vault.quote mutates the simulated pool. Keep it out of the payment frame.
        quoted=simulate([*prefix,*approval,probe])[-1]
        def attempt(floor, end):
            op=packet(mode,roles,pool,source,target,qty,end,floor)
            values=simulate([*prefix,*approval,*checks,op,*checks])
            prep=values[:len(prefix)+len(approval)]
            if any(x["status"]!="0x1" for x in prep):
                return None,{"stage":"funding_or_approval","error":next(x.get("error") for x in prep if x["status"]!="0x1")}
            rest=values[len(prep):]
            if any(x["status"]!="0x1" or len(x["returnData"])!=66 for x in [*rest[:4],*rest[5:]]):
                raise ValueError(("balance read",pool,source,target))
            b,a=([int(x["returnData"],16) for x in z] for z in (rest[:4],rest[5:]))
            return (op,rest[4],b,a,quoted),None
        trial,err=attempt(1,deadline)
        if err:return None,err
        op,result,b,a,quote=trial
        if result["status"]!="0x1":
            return None,{"stage":"swap","error":result.get("error"),"quote":quote.get("returnData")}
        paid, credit = b[0]-a[0], a[1]-b[1]
        if paid!=qty or credit<=0 or b[2:]!=a[2:]:
            raise ValueError(("wallet delta",mode,pool,qty,paid,credit))
        checked,_=attempt(credit,deadline)
        if checked is None or checked[1]["status"]!="0x1" or checked[3]!=a:
            raise ValueError(("exact minimum",mode,pool,qty,credit))
        for floor,end in ((credit+1,deadline),(1,0)):
            rejected,_=attempt(floor,end)
            if rejected is None or rejected[1]["status"]!="0x0" or rejected[2]!=rejected[3]:
                raise ValueError(("failed protection",mode,pool,source,floor,end))
        return [*prefix,*approval,checked[0]],dict(stage="paid",debit=paid,credit=credit,
             quote=int(quote["returnData"],16) if quote["status"]=="0x1" else quote.get("error"),
             gas=int(result["gasUsed"],16),min_plus_one="reverted",expired="reverted")
    # All four shards cover each initialized pool with funded cash. Exercise every nonempty target for one cash.
    plans=[]
    for idx,p in enumerate(directory["pools"]):
        if idx%4!=number or not p["initialized"] or p["paused"] or p["recovery"] or p["hook_stopped"]:continue
        cash=next((t for t in SIZES if t in p["tokens"] and p["raw"][p["tokens"].index(t)]),None)
        if cash:
            plans.extend((p["pool"],cash,t) for t,q in zip(p["tokens"],p["raw"]) if t!=cash and q>0)
    def one(plan):
        pool,cash,token=plan
        records=[]
        for qty in SIZES[cash]:
            for mode in ("router","v5"):
                prefix,buy=verify(mode,pool,cash,token,qty,[buy_cash(cash,qty,deadline)])
                row=dict(pool=pool,cash=cash,token=token,mode=mode,size=qty,buy=buy)
                if prefix:
                    _,row["sell"]=verify(mode,pool,token,cash,buy["credit"],prefix)
                records.append(row)
        return records
    with ThreadPoolExecutor(max_workers=3) as workers:
        rows=[x for group in workers.map(one,plans) for x in group]
    if plans and not any(x["buy"]["stage"]=="paid" for x in rows):
        raise ValueError(("no wallet payout",rows[:2]))
    return dict(selector=AT,state_root=ROOT_HASH,chain=1,shard=number,payer=OWNER,executor=EXEC,cases=rows)


if __name__ == "__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("out",type=Path)
    p.add_argument("shard",type=int,choices=range(4))
    args=p.parse_args()
    for endpoint in [ENDPOINT,"https://ethereum-rpc.publicnode.com","https://mainnet.gateway.tenderly.co"]:
        try:
            evidence=run(endpoint,args.shard)
            break
        except (OSError, RuntimeError, KeyError, ValueError) as exc:
            if isinstance(exc, ValueError) and isinstance(exc.args[0],tuple): raise
            print(endpoint, str(exc)[:220])
    else: raise RuntimeError("no archive RPC supplied complete canonical calls")
    args.out.write_text(json.dumps(evidence,separators=(",",":")),encoding="utf8")
    print(len(evidence["cases"]), "market, amount and settlement combinations")
