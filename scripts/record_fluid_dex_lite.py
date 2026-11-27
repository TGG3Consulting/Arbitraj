"""Read Fluid DEX Lite at one Ethereum parent and replay independently funded swaps.

The deployment, ABI and Solidity storage layout are published by Instadapp in
fluid-contracts-public@404d73f91e6806f881f8dabeb30494f93f9e99b7/deployments/mainnet.
The DefiLlama volume adapter uses a different resolver. Reconcile all three
directories with the core DexKey array before using any quotes.
"""

import json
import sys
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_aktionariat_wallet import FUND, SWAP, WETH, approve
from record_balancer_markets import Capture
from record_base_slipstream_executor import OWNER, ROOT, rpc, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx

CORE = "0xbbcb91440523216e2b87052a99f69c604a7b6e00"
RESOLVER = "0x99b19a93ba0f516b173eef53d07b3253fbcfd564"
LENSES = (RESOLVER, "0x12a47ceb96a952e8d4a6ea9fe3b40b79bbaeb4e9", "0x26b696d0dfdab6c894aa9a6575fcd07bb25bbd2c")
USDC = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
USDE = "0x4c9edd5852cd905f086c759e8383e09bff1e68b3"
DEX_KEY = "(address,address,bytes32)"
QUOTE = [DEX_KEY, "bool", "int256"]
SWAP_ARGS = [*QUOTE, "uint256", "address", "bool", "bytes", "bytes"]
LOG_SWAP = "0x" + keccak(text="LogSwap(uint256,uint256)").hex()


def method(name, types):
    return name + "(" + ",".join(types) + ")"


def independently_fund(token, end):
    # All tokens reach the isolated wallet via ordinary V3 swaps in unrelated pools.
    start = 10**17
    first = USDC if token == USDE else token
    if first not in (USDC, USDT):
        raise ValueError(f"no independent funding route for {token}")
    def buy(a, b, fee, output, limit):
        return tx(FUND, "exactOutputSingle(" + SWAP + ")", [SWAP], [(a, b, fee, OWNER, end, output, limit, 0)])
    prep = [tx(WETH, "deposit()", value=start), approve(WETH, FUND, start), buy(WETH, first, 500, 20_000_000, start)]
    if token == USDE:
        prep.extend([approve(USDC, FUND, 20_000_000), buy(USDC, USDE, 100, 4 * 10**18, 20_000_000)])
    decimals = 18 if token == USDE else 6
    return prep + [approve(token, CORE, 4 * 10**decimals)]


def pay(c, key, forward, exact_input, end):
    source, target = key[:2] if forward else key[1::-1]
    fixed = source if exact_input else target
    qty = 10**(18 if fixed == USDE else 6)
    request = [key, forward, qty if exact_input else -qty]
    q = tx(RESOLVER, method("estimateSwapSingle", QUOTE), QUOTE, request)
    prep = independently_fund(source, end)
    owners = {OWNER: {"balance": hex(10**18)}}
    def replay(calls):
        return simulate_block(c.endpoint, c.selector, owners, calls)
    quote_run = replay(prep + [q])
    if any(row["status"] != "0x1" for row in quote_run[:-1]):
        raise ValueError(("funding failed", source, [(i,r.get("error")) for i,r in enumerate(quote_run[:-1]) if r["status"] != "0x1"]))
    if quote_run[-1]["status"] != "0x1":
        return dict(source=source,target=target,exact_input=exact_input,requested=str(qty),quote_error=quote_run[-1].get("error"))
    price = int(quote_run[-1]["returnData"], 16)
    assert price > 0
    def order(limit):
        return tx(CORE, method("swapSingle", SWAP_ARGS), SWAP_ARGS, [*request, limit, OWNER, False, b"", b""])
    bal = [balance(source, OWNER), balance(target, OWNER)]
    attempt = order(price)
    stream = prep + [q] + bal + [order(price + (1 if exact_input else -1))] + bal + [attempt] + bal
    out = replay(stream)
    n = len(prep) + 1
    failures = [(i,r.get("error")) for i,r in enumerate(out) if r["status"] != "0x1"]
    if [i for i,_ in failures] != [n+2]:
        raise ValueError(("unexpected swap result", source, target, exact_input, price, failures))
    error = failures[0][1]
    reason = "AmountLimitNotMet" if exact_input else "AmountLimitExceeded"
    assert error["data"][:10] == "0x" + keccak(text=reason+"(uint256,uint256)")[:4].hex(), error
    assert int(out[n-1]["returnData"],16) == price
    spent, received = (qty, price) if exact_input else (price, qty)
    def balances(i):
        return [int(x["returnData"], 16) for x in out[i:i+2]]
    before, middle, after = balances(n), balances(n+3), balances(n+6)
    assert middle == before and [before[0]-after[0], after[1]-before[1]] == [spent, received]
    paid = out[n+5]
    assert int(paid["returnData"],16) == price
    assert (token_flow(paid["logs"],source,OWNER),token_flow(paid["logs"],target,OWNER)) == (-spent,received)
    matching = [x for x in paid["logs"] if x["address"].lower()==CORE and x["topics"]==[LOG_SWAP]]
    assert len(matching)==1
    swap_data,_ = decode(["uint256","uint256"],bytes.fromhex(matching[0]["data"][2:]))
    pool_id = keccak(encode([DEX_KEY],[key]))[:8]
    assert (swap_data & ((1 << 64)-1)) == int.from_bytes(pool_id,"big")
    assert bool((swap_data >> 64) & 1) == forward
    return dict(source=source,target=target,exact_input=exact_input,requested=str(qty),spent=str(spent),received=str(received),
                tx=attempt,logs=paid["logs"],gas=int(paid["gasUsed"],16),strict=error)


def acquire(url):
    cfg = json.loads((ROOT / "tests/data/base_ethereum_networks.json").read_bytes())["ethereum"]
    tag = {"blockHash": cfg["hash"], "requireCanonical": True}
    header = rpc(url, "eth_getBlockByHash", [cfg["hash"], False])
    assert header["hash"] == cfg["hash"] and int(header["number"], 16) == cfg["block"] and int(rpc(url,"eth_chainId",[]),16)==1
    c = Capture(dict(endpoint=url,selector=tag))
    def storage(i):
        return c.one(CORE,"readFromStorage(bytes32)",["bytes32"],[i.to_bytes(32,"big")],["uint256"])[0].to_bytes(32,"big")
    count = int.from_bytes(storage(1),"big")
    start = int.from_bytes(keccak((1).to_bytes(32,"big")),"big")
    markets = []
    for index in range(count):
        words = b"".join(storage(start+3*index+j) for j in range(3))
        markets.append(decode([DEX_KEY],words)[0])
    assert markets and len(set(markets))==len(markets)
    for lens in LENSES:
        assert c.one(lens,"getAllDexes()",returns=[DEX_KEY+"[]"])[0] == tuple(markets)
    roles = {}
    for address in (CORE,*LENSES):
        code = bytes.fromhex(rpc(url,"eth_getCode",[address,tag])[2:])
        assert code
        roles[address] = "0x" + keccak(code).hex()
    results = []
    for key in markets:
        values = c.one(RESOLVER,method("getPricesAndReserves",[DEX_KEY]),[DEX_KEY],[key],
                       ["(uint256,uint256,uint256,uint256,uint256,uint256)", "(uint256,uint256,uint256,uint256)"])
        payments = [pay(c,key,direction,exact_input,int(header["timestamp"],16)+600)
                    for direction in (True,False) for exact_input in (True,False)]
        results.append(dict(token0=key[0],token1=key[1],salt="0x"+key[2].hex(),pool_id="0x"+keccak(encode([DEX_KEY],[key]))[:8].hex(),
                            prices=[str(v) for v in values[0]],reserves=[str(v) for v in values[1]],payments=payments))
    return dict(height=cfg["block"],head=tag,roles=roles,pools=results)


if __name__ == "__main__":
    for url in ("https://eth.drpc.org", "https://ethereum-rpc.publicnode.com", "https://rpc.mevblocker.io"):
        try:
            report = acquire(url)
            break
        except (requests.RequestException, RuntimeError) as err:
            print(url, str(err)[:200], flush=True)
    else:
        raise RuntimeError("Ethereum Fluid DEX Lite archive unavailable")
    Path(sys.argv[1]).write_text(json.dumps(report,separators=(",",":")),encoding="utf8")
