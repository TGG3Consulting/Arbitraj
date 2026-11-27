"""Measure ordinary Ammalgam spot swaps independently of its six lending token balances."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak

from record_aktionariat_wallet import FUND, SWAP, WETH, approve
from record_balancer_markets import Capture
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, rpc, simulate_block
from record_bitswap_ethereum import balance, read_batch, token_flow, tx

# First-party production indexer and published mainnet frontend, https://app.ammalgam.xyz/trade
INDEX = "https://github.com/Ammalgam-Protocol/subgraph/blob/247d6aaa58052b8af779e98947cc3dce14976700/config.mainnet.yaml"
APP = "https://app.ammalgam.xyz/_next/static/chunks/pages/_app-7c7f3ed770b5c364.js"
APP_SHA256 = "b5bf065230bea1f47ab44bdf03b13f795148c995fb8d79d8aae0e94a5c416508"
FACTORY = "0x1a411b0fd1f368d2f413a8cbb6aad425c923015b"
HELPER = "0x910068dfaac6aab0847057aff323045f46dd8468"
USDC = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
SWAP_ARGS = "(address,address,address,uint256,uint256)"
IMPL_SLOT = hex(int.from_bytes(keccak(text="eip1967.proxy.implementation"), "big") - 1)
BEACON_SLOT = hex(int.from_bytes(keccak(text="eip1967.proxy.beacon"), "big") - 1)


def capture(url, compiled):
    source = json.loads((ROOT / "tests/data/base_ethereum_networks.json").read_bytes())["ethereum"]
    tag = {"blockHash": source["hash"], "requireCanonical": True}
    header = rpc(url, "eth_getBlockByHash", [tag["blockHash"], False])
    assert rpc(url, "eth_chainId", []) == "0x1" and int(header["number"], 16) == source["block"]
    c = Capture(dict(selector=tag, endpoint=url))
    end = int(header["timestamp"], 16) + 900

    def code(target):
        raw = rpc(url, "eth_getCode", [target, tag])
        assert raw != "0x"
        return "0x" + keccak(hexstr=raw).hex()

    def one(target, method, returns, types=(), values=()):
        return c.one(target, method, types, values, returns)

    def binding(target):
        slot, direct = read_batch(url, [("eth_getStorageAt", [target, s, tag]) for s in (BEACON_SLOT, IMPL_SLOT)])
        record = dict(runtime=code(target))
        if int(slot, 16):
            beacon = "0x" + slot[-40:]
            impl, = one(beacon, "implementation()", ["address"])
            record.update(beacon=beacon, beacon_hash=code(beacon), implementation=impl, implementation_hash=code(impl))
        # PairBeaconProxy stores both the beacon and the last initialized implementation.
        # Its fallback may use the previous implementation for a read during an upgrade.
        if int(direct, 16) and int(slot, 16):
            current = "0x" + direct[-40:]
            record.update(initialized=current, initialized_hash=code(current), synchronized=impl == current)
        elif int(direct, 16):
            impl = "0x" + direct[-40:]
            record.update(implementation=impl, implementation_hash=code(impl))
        return record

    pools = []
    count, = one(FACTORY, "allPairsLength()", ["uint256"])
    for index in range(count):
        pool, = one(FACTORY, "allPairs(uint256)", ["address"], ["uint256"], [index])
        coins = one(pool, "underlyingTokens()", ["address", "address"])
        assert all(one(FACTORY, "getPair(address,address)", ["address"], ["address", "address"], pair) == (pool,)
                   for pair in (coins, coins[::-1]))
        reserves = one(pool, "getReserves()", ["uint112", "uint112", "uint32"])
        assets, shares = one(pool, "totalAssetsAndShares(bool)", ["uint112[6]", "uint112[6]"], ["bool"], [True])
        cash = [one(t, "balanceOf(address)", ["uint256"], ["address"], [pool])[0] for t in coins]
        pinned = binding(pool)
        assert pinned.get("beacon") == one(FACTORY, "pairBeacon()", ["address"])[0]
        pools.append(dict(index=index, pool=pool, tokens=coins, code=pinned, reserves=list(map(str,reserves)),
                          cash=list(map(str,cash)), lending_assets=list(map(str,assets)), lending_shares=list(map(str,shares))))
    assert len({p["pool"] for p in pools}) == count
    market, = [p for p in pools if set(p["tokens"]) == {WETH, USDC}]
    data = dict(selector=tag, height=source["block"], state_root=header["stateRoot"], factory=FACTORY, helper=HELPER,
                factory_code=binding(FACTORY), helper_code=binding(HELPER), pools=pools, controls=[],
                origin=dict(factory=INDEX, wallet_interface=APP, frontend_sha256=APP_SHA256))
    env = {OWNER: {"balance": hex(5 * 10**18)}}
    assert rpc(url, "eth_getCode", [OWNER, tag]) == "0x"
    module = "0x" + keccak(bytes.fromhex("d694" + OWNER[2:] + "80"))[-20:].hex()
    prefix = []
    if compiled:
        assert rpc(url, "eth_getTransactionCount", [OWNER, tag]) == "0x0"
        assert all(rpc(url, "eth_getCode", [a, tag]) == "0x" for a in (EXEC,module))
        args = encode(["address"]*3, [FACTORY,HELPER,market["pool"]])
        deploy = {"from": OWNER, "data": "0x"+compiled["creation"]+args.hex(), "gas": hex(6000000)}
        word = lambda n: "0x"+format(n,"064x")
        env[EXEC] = {"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
                     "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}
        created, = simulate_block(url,tag,env,[deploy])
        assert created["status"] == "0x1" and created["returnData"] != "0x"
        data["module"] = dict(address=module, runtime="0x"+keccak(hexstr=created["returnData"]).hex(),
                              creation=deploy["data"], constructor=[FACTORY,HELPER,market["pool"]],
                              gas=int(created["gasUsed"],16))
        prefix = [deploy]
    stable = "0xdac17f958d2ee523a2206206994597c13d831ec7"
    routes = [(WETH,USDC,(10**15,10**16)),(USDC,WETH,(10**6,10**7)),
              (USDC,stable,(10**6,10**7)),(stable,USDC,(10**6,10**7))]
    for mode in (("helper","wallet","v5") if compiled else ("helper",)):
        for src,dst,amounts in routes:
            market, = [p for p in pools if set(p["tokens"]) == {src,dst}]
            spender = dict(helper=HELPER, wallet=module, v5=EXEC)[mode]
            for amount in amounts:
                seed = 2 * 10**17
                prep = prefix + [tx(WETH, "deposit()", value=seed)]
                if src != WETH:
                    prep += [approve(WETH, FUND, seed), tx(FUND, "exactInputSingle("+SWAP+")", [SWAP],
                               [(WETH, src, 3000, OWNER, end, seed, amount, 0)])]
                prep += [approve(src, spender, amount)]
                before = [balance(src, OWNER), balance(dst, OWNER)]
                def swap(floor,deadline=end):
                    if mode == "v5":
                        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                                  ["address","uint256","address","uint256","uint256",STEP+"[]"],
                                  [src,amount,dst,floor,deadline,[(29,module,src,dst,10000,b"")]])
                    if mode == "wallet":
                        return tx(module,"execute(address,address,address,uint256,uint256,uint256)",
                                  ["address"]*3+["uint256"]*3,[market["pool"],src,dst,amount,floor,deadline])
                    return tx(HELPER, "swapHelper("+SWAP_ARGS+")", [SWAP_ARGS], [(OWNER, src, dst, amount, floor)])
                at = len(prep)
                def replay(operations):
                    result = simulate_block(url, tag, env, prep + before + operations + before)
                    assert all(q["status"] == "0x1" for q in result[:at+2] + result[-2:])
                    assert int(result[at]["returnData"],16) >= amount
                    return result
                result = replay([swap(1)])
                call = result[at+2]
                row = dict(mode=mode, pool=market["pool"], input=src, output=dst, amount=str(amount))
                data["controls"].append(row)
                if call["status"] != "0x1":
                    row["refusal"] = call["error"]
                    assert [x["returnData"] for x in result[at:at+2]] == [x["returnData"] for x in result[-2:]]
                    continue
                spent = int(result[at]["returnData"],16) - int(result[-2]["returnData"],16)
                received = int(result[-1]["returnData"],16) - int(result[at+1]["returnData"],16)
                assert spent == amount and received > 0
                assert token_flow(call["logs"], src, OWNER) == -spent and token_flow(call["logs"], dst, OWNER) == received
                assert any(l["address"].lower() == market["pool"] and l["topics"][0] == "0x"+keccak(text="Swap(address,uint256,uint256,uint256,uint256,address)").hex() for l in call["logs"])
                rejects = [swap(received+1)]
                if mode in ("wallet","v5"):
                    rejects.append(swap(received,1))
                checked = replay(rejects + [swap(received)])
                assert all(q["status"] == "0x0" for q in checked[at+2:at+2+len(rejects)])
                assert checked[at+2+len(rejects)]["status"] == "0x1"
                row["expired"] = checked[at+3]["error"] if len(rejects)>1 else None
                assert [x["returnData"] for x in checked[-2:]] == [x["returnData"] for x in result[-2:]]
                row.update(spent=str(spent), received=str(received), gas=int(call["gasUsed"],16),
                           minimum_refusal=checked[at+2]["error"], logs=call["logs"])
    if compiled:
        for c in data["controls"]:
            other = next(o for o in data["controls"] if o["mode"]=="helper" and
                         (o["input"],o["output"],o["amount"])==(c["input"],c["output"],c["amount"]))
            if "received" in other:
                assert (other["received"],other["spent"]) == (c.get("received"),c.get("spent")), (c["mode"],c["pool"],c.get("refusal"))
    return data


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--compiled",type=Path)
    args = parser.parse_args()
    for endpoint in ("https://rpc.mevblocker.io", "https://eth.drpc.org"):
        try:
            snapshot = capture(endpoint, json.loads(args.compiled.read_bytes()) if args.compiled else None)
            break
        except (requests.RequestException, ValueError) as error:
            last = error
    else:
        raise RuntimeError("No archive returned a complete pinned read") from last
    args.output.write_text(json.dumps(snapshot, separators=(",", ":")))
