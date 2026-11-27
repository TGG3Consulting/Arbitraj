"""Оплаченный замкнутый путь в физических USDT через два разных WETH/USDC рынка Base."""
import json
import sys
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak

from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, call, calldata, rpc, simulate_block
from record_early_slipstream import WETH, approval, balance
from record_omni_v2_base import ENDPOINTS

DATA = ROOT / "tests/data"
USDT = "0xfde4c96c8593536e31f229ea8f37b2ada2699bb2"
USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
TRANSFER = "0x" + keccak(text="Transfer(address,address,uint256)").hex()


def execute(source, target, amount, limit, deadline, steps):
    data = calldata("execute(address,uint256,address,uint256,uint256," + STEP + "[])",
                    ["address", "uint256", "address", "uint256", "uint256", STEP + "[]"],
                    [source, amount, target, limit, deadline, steps])
    return call(EXEC, data, gas=2_500_000)


def main(path):
    config = json.loads((DATA/"base_slipstream_2026-11-26/quotes.json").read_text())
    tag = config["selector"]
    directory = DATA/"executor_v5_base_2026-11-26"
    keys = json.loads((directory/"keys.json").read_text())
    manager = json.loads((directory/"summary.json").read_text())[0]["manager"]
    profiles = json.loads((ROOT/"config/first_run_dex.json").read_text())
    factory = next(p["settings"]["dex.factory"] for p in profiles if p["id"]=="uniswap-v3-base")
    word = lambda n: "0x"+format(n, "064x")
    runtime = RUNTIME.read_text().strip().removeprefix("0x")
    overrides = {OWNER:{"balance":hex(2*10**18)},EXEC:{"code":"0x"+runtime,
                 "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    markets = [r["pool"] for r in sorted(config["pools"],key=lambda r:int(r["liquidity"]),reverse=True)[:2]]
    providers = []
    for url in ENDPOINTS:
        try:
            head = rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
            assert head["hash"]==tag["blockHash"] and int(head["number"],16)==config["block"]
            assert int(rpc(url,"eth_chainId",[]),16)==8453
            assert all(rpc(url,"eth_getCode",[a,tag])=="0x" for a in [EXEC,OWNER])
            assert int(rpc(url,"eth_getTransactionCount",[OWNER,tag]),16)==0
            assert "0x"+keccak(bytes.fromhex(rpc(url,"eth_getCode",[USDT,tag])[2:])).hex()==next(
                p["venue_settings"]["dex_dex.bridged_usdt"].split()[-1] for p in profiles if p["id"]=="uniswap-v4-base")
            calls=[call(USDT,calldata(s)) for s in ["l1Token()","l2Bridge()","decimals()","symbol()"]]
            calls += [call(factory,calldata("getPool(address,address,uint24)",["address","address","uint24"],
                                            [WETH,USDT,tier])) for tier in (500,3000,100,10000)]
            rows = simulate_block(url,tag,overrides,calls)
            assert all(int(r["status"],16)==1 for r in rows)
            answers=[decode([t],bytes.fromhex(r["returnData"][2:]))[0] for t,r in zip(
                ["address","address","uint8","string"],rows[:4],strict=True)]
            assert answers==["0xdac17f958d2ee523a2206206994597c13d831ec7","0x4200000000000000000000000000000000000010",6,"USDT"]
            pools=[decode(["address"],bytes.fromhex(r["returnData"][2:]))[0] for r in rows[4:]]
            pools=[p for p in pools if int(p,16)]
            holdings=simulate_block(url,tag,overrides,[balance(USDT,p) for p in pools])
            seed=max(zip(pools,holdings,strict=True),key=lambda pair:int(pair[1]["returnData"],16))[0]
            providers.append((url,head,seed))
        except (ValueError,OSError,KeyError,RuntimeError,AssertionError,TypeError) as e:
            print(url,type(e).__name__,str(e)[:150],flush=True)
    assert providers, "Нет подтверждённого архивного узла с собственным источником USDT"
    _,head,seed=providers[0]
    assert all(h["stateRoot"]==head["stateRoot"] and p==seed for _,h,p in providers)
    deadline=int(head["timestamp"],16)+600
    def step(pool,x,y):
        return (1,pool,x,y,10000,b"")
    def convert(key,x,y):
        return (19,manager,x,y,10000,bytes.fromhex(key["poolKey"][2:]))
    funding=10**18
    prefix=[call(WETH,calldata("deposit()"),funding), approval(WETH,EXEC,funding),
            execute(WETH,USDT,funding,100*10**6,deadline,[step(seed,WETH,USDT)])]
    assets=[USDT,USDC,WETH]
    checks=[balance(t,a) for a in (OWNER,EXEC) for t in assets]
    def read(url,amount,steps,minimum):
        tx=execute(USDT,USDT,amount,minimum,deadline,steps)
        before=prefix+[approval(USDT,EXEC,amount)]+checks
        rc=simulate_block(url,tag,overrides,before+[tx]+checks)
        trade=rc[len(before)]
        assert all(int(r["status"],16)==1 for r in rc[:len(before)]+rc[len(before)+1:])
        pre=[int(r["returnData"],16) for r in rc[len(before)-len(checks):len(before)]]
        post=[int(r["returnData"],16) for r in rc[-len(checks):]]
        return trade,pre,post,tx,int(rc[3]["gasUsed"],16)
    recorded=[]
    for a,b in [markets,markets[::-1]]:
        for first,last in [(keys[0],keys[0]),(keys[0],keys[1]),(keys[1],keys[0]),(keys[1],keys[1])]:
            for amount in [10**6,100*10**6]:
                steps=[convert(first,USDT,USDC),step(a,USDC,WETH),step(b,WETH,USDC),convert(last,USDC,USDT)]
                for url,_,_ in providers:
                    try:
                        trade,pre,post,_,approval_gas=read(url,amount,steps,1)
                        assert int(trade["status"],16)==1,(trade.get("error"),a,b,amount)
                        got=int(trade["returnData"],16)
                        exact,p0,p1,tx,_=read(url,amount,steps,got)
                        above,r0,r1,_,_=read(url,amount,steps,got+1)
                        break
                    except (ValueError,OSError) as e:
                        print(url,type(e).__name__,str(e)[:120],flush=True)
                else:
                    raise RuntimeError("Серия archive RPC не завершилась")
                assert p0==pre==r0==r1 and p1==post and post[1:]==pre[1:]
                assert post[0]-pre[0]==got-amount and int(exact["status"],16)==1 and int(above["status"],16)==0
                assert int(exact["returnData"],16)==got and not above["logs"]
                flows=[(l["topics"][1][-40:].lower(),l["topics"][2][-40:].lower(),int(l["data"],16))
                       for l in exact["logs"] if l["address"].lower()==USDT and l["topics"][0]==TRANSFER]
                assert sum(n for src,_,n in flows if src==OWNER[2:])==amount
                assert sum(n for _,dst,n in flows if dst==OWNER[2:])==got
                recorded.append(dict(pools=[first["pool_id"],a,b,last["pool_id"]],spent=str(amount),received=str(got),
                    before=list(map(str,pre)),after=list(map(str,post)),gas=int(exact["gasUsed"],16),
                    approval_gas=approval_gas,above=above,exact=exact,transaction=tx))
    path.write_text(json.dumps(dict(selector=tag,block=config["block"],stateRoot=head["stateRoot"],
        owner=OWNER,executor=EXEC,runtime="0x"+keccak(bytes.fromhex(runtime)).hex(),funding_pool=seed,
        cash=USDT,working=USDC,assets=assets,bridge_getters=answers,cases=recorded))+"\n")


if __name__=="__main__":
    main(Path(sys.argv[1]))
