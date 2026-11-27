"""Сверка ранней фабрики Slipstream по собственному финансированию и каноническому H Base."""
import json
import sys
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak

from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, call, calldata, rpc, simulate_block
from record_omni_v2_base import ENDPOINTS

DIRECTORY = ROOT / "tests/data/first_run_dex_2026-11-26"
WETH = "0x4200000000000000000000000000000000000006"
PERMIT2 = "0x000000000022d473030f116ddee9f6b43ac78ba3"
SWAP = "0x" + keccak(text="Swap(address,address,int256,int256,uint160,uint128,int24)").hex()


def approval(token, spender, amount):
    return call(token, calldata("approve(address,uint256)", ["address", "uint256"], [spender, amount]))


def balance(token, account):
    return call(token, calldata("balanceOf(address)", ["address"], [account]))


def execute(pool, asset, target, amount, minimum, deadline):
    args = [asset, amount, target, minimum, deadline, [(1, pool, asset, target, 10000, b"")]]
    return call(EXEC, calldata("execute(address,uint256,address,uint256,uint256," + STEP + "[])",
        ["address", "uint256", "address", "uint256", "uint256", STEP + "[]"], args))


def router_swap(router, asset, target, spacing, amount, minimum, deadline):
    path = bytes.fromhex(asset[2:]) + spacing.to_bytes(3, "big", signed=True) + bytes.fromhex(target[2:])
    inner = encode(["address", "uint256", "uint256", "bytes", "bool"], [OWNER, amount, minimum, path, True])
    return call(router, calldata("execute(bytes,bytes[],uint256)", ["bytes", "bytes[]", "uint256"],
                                [bytes([0]), [inner], deadline]))


def scenario(url, tag, override, roles, market, funding, row, mode, deadline):
    asset, amount = row["input_token"], int(row["input_raw"])
    pool, tokens, spacing = market["pool"], market["tokens"], int(market["tick_spacing"])
    target = next(t for t in tokens if t.lower() != asset.lower())
    wrapped = asset.lower() == WETH
    size = amount if wrapped else 10**17
    prefix = [("wrap", call(WETH, calldata("deposit()"), size))]
    if not wrapped:
        assert funding["pool"].lower() != pool.lower()
        prefix.extend([("allow_funding", approval(WETH, EXEC, size)),
                       ("fund_other_pair", execute(funding["pool"], WETH, asset, size, amount, deadline))])
    router = roles["router"]["address"]
    wallet = EXEC if mode == "V5" else router
    prefix.append(("allow", approval(asset, EXEC if mode == "V5" else PERMIT2, amount)))
    if mode != "V5":
        prefix.append(("allow_permit", call(PERMIT2, calldata("approve(address,address,uint160,uint48)",
                      ["address", "address", "uint160", "uint48"], [asset, router, amount, deadline]))))
    args = (asset, target, amount, spacing, 0)
    prefix.append(("quote", call(roles["quoter"]["address"], calldata(
        "quoteExactInputSingle((address,address,uint256,int24,uint160))",
        ["(address,address,uint256,int24,uint160)"], [args]))))
    balances = [("payer_input", balance(asset, OWNER)), ("payer_output", balance(target, OWNER)),
                ("wallet_input", balance(asset, wallet)), ("wallet_output", balance(target, wallet))]
    def swap(minimum, limit):
        if mode == "V5":
            return execute(pool, asset, target, amount, minimum, limit)
        return router_swap(router, asset, target, spacing, amount, minimum, limit)
    def run(controls):
        named = prefix + [("pre_"+k, c) for k,c in balances] + controls + [("post_"+k,c) for k,c in balances]
        raw = simulate_block(url, tag, override, [c for _,c in named])
        results = {k:r for (k,_),r in zip(named, raw, strict=True)}
        for k,_ in prefix + [("pre_"+k,c) for k,c in balances] + [("post_"+k,c) for k,c in balances]:
            if int(results[k]["status"],16) != 1:
                raise AssertionError((pool, mode, k, results[k].get("error")))
        # Last word is the measured quoter gas; the child block can update an observation.
        actual = results["quote"]["returnData"].lower()
        assert len(actual) == len(row["return_data"]) == 258 and actual[:194] == row["return_data"].lower()[:194]
        return results
    def number(results, k):
        return int(results[k]["returnData"],16)
    initial = run([("trade",swap(1,deadline))])
    if int(initial["trade"]["status"],16) != 1:
        assert all(number(initial,"pre_"+k) == number(initial,"post_"+k) for k,_ in balances)
        return dict(pool=pool,asset=asset,amount=str(amount),mode=mode,quote=row["return_data"],
                    trade=initial["trade"],full=False)
    spent = number(initial,"pre_payer_input") - number(initial,"post_payer_input")
    net = number(initial,"post_payer_output") - number(initial,"pre_payer_output")
    assert 0 < spent <= amount and net > 0
    assert any(l["address"].lower() == pool.lower() and l["topics"][0].lower() == SWAP for l in initial["trade"]["logs"])
    controls = [("expired",swap(net,1)),("tighter",swap(net+1,deadline)),("exact",swap(net,deadline))]
    checked = run(controls)
    assert int(checked["expired"]["status"],16) == 0
    stricter = checked["tighter"]
    if int(stricter["status"],16) == 1:
        assert mode != "V5"
        checked = run([controls[-1]])
    assert int(checked["exact"]["status"],16) == 1
    assert all(number(checked,"post_"+k) == number(initial,"post_"+k) for k,_ in balances)
    return dict(pool=pool,asset=asset,amount=str(amount),mode=mode,quote=row["return_data"],
                spent=str(spent),received=str(net),full=spent==amount,
                retained_input=str(number(initial,"post_wallet_input")-number(initial,"pre_wallet_input")),
                retained_output=str(number(initial,"post_wallet_output")-number(initial,"pre_wallet_output")),
                approval_gas=sum(int(initial[k]["gasUsed"],16) for k in ("allow","allow_permit") if k in initial),
                gas=int(checked["exact"]["gasUsed"],16),trade=checked["exact"],minimum_plus_one=stricter,
                exact_transaction=controls[-1][1],funding_pool=None if wrapped else funding["pool"])


def main(path):
    config = json.loads((DIRECTORY/"early_slipstream_base.json").read_text("utf8"))
    tag, roles = config["selector"], config["roles"]
    cases = config["quotes"]
    markets = config["markets"]
    word = lambda n: "0x"+format(n,"064x")
    runtime = RUNTIME.read_text().strip().removeprefix("0x")
    override = {OWNER:{"balance":hex(10**18)},EXEC:{"code":"0x"+runtime,"stateDiff":{
        word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    providers = []
    for url in ENDPOINTS:
        try:
            assert int(rpc(url,"eth_chainId",[]),16)==8453
            head = rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
            assert head["hash"]==tag["blockHash"] and int(head["number"],16)==62262756
            for role in roles.values():
                code = rpc(url,"eth_getCode",[role["address"],tag])
                assert "0x"+keccak(bytes.fromhex(code[2:])).hex()==role["runtime"]
            assert all(rpc(url,"eth_getCode",[a,tag])=="0x" for a in (EXEC,OWNER))
            simulate_block(url,tag,override,[balance(WETH,OWNER)])
            providers.append((url,head))
        except (ValueError,RuntimeError,AssertionError,OSError,TypeError,KeyError) as e:
            print(url,type(e).__name__,str(e)[:160],flush=True)
    assert providers, "Нет подтверждённого архивного узла"
    head = providers[0][1]
    assert all(h["stateRoot"]==head["stateRoot"] for _,h in providers)
    output = []
    for row in cases:
        pool = row["pool"]
        for mode in ["V5","UniversalRouter"]:
            for url,_ in providers:
                try:
                    result=scenario(url,tag,override,roles,markets[pool],config["funding"][pool],
                                    row,mode,int(head["timestamp"],16)+600)
                    break
                except (ValueError,OSError) as e:
                    print(url,mode,type(e).__name__,str(e)[:160],flush=True)
            else:
                raise RuntimeError("Все архивные узлы недоступны")
            output.append(result)
            print(pool,mode,result.get("spent"),result.get("received"),flush=True)
    for pool in markets:
        for asset in markets[pool]["tokens"]:
            assert any(r["pool"]==pool and r["asset"].lower()==asset.lower() and r["full"] and
                       r["mode"]=="V5" for r in output), (pool,asset,"нет полностью оплаченного объёма")
    path.write_text(json.dumps(dict(selector=tag,stateRoot=head["stateRoot"],owner=OWNER,executor=EXEC,
                    runtime="0x"+keccak(bytes.fromhex(runtime)).hex(),cases=output),indent=2)+"\n",encoding="utf8")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
