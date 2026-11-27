"""Record original V1 exact-input/output price operands at the Ethereum deployment snapshot (read-only)."""
import argparse
import json
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak

from record_balancer_markets import Capture, posts
from record_uniswap_v1 import DAI, FACTORY, MULTI, ROOT, STABLES, WETH


def capture(endpoint):
    snapshot = json.loads((ROOT / 'tests/data/first_run_dex_2026-11-26/ethereum.json').read_bytes())
    c = Capture(dict(snapshot, endpoint=endpoint))
    def rpc(method, params):
        r = c.session.post(endpoint, json=dict(id=1,jsonrpc="2.0",method=method,params=params), timeout=18)
        r.raise_for_status()
        return r.json()["result"]
    head = rpc("eth_getBlockByHash", [c.selector["blockHash"],False])
    if head["hash"] != c.selector["blockHash"] or int(head["number"],16) != snapshot["block"] or rpc("eth_chainId",[]) != "0x1":
        raise ValueError("unexpected network or block")
    def code(address):
        raw = bytes.fromhex(rpc("eth_getCode",[address,c.selector])[2:])
        if not raw:
            raise ValueError(("missing code", address))
        return dict(address=address, bytes=len(raw), keccak256="0x"+keccak(raw).hex())
    template, = c.one(FACTORY, "exchangeTemplate()",[],[],["address"])
    roles = {"factory":code(FACTORY), "template":code(template)}
    tokens = [*STABLES, DAI, WETH]
    markets = {}
    for token in tokens:
        exchange, = c.one(FACTORY, "getExchange(address)", ["address"], [token], ["address"])
        facts = posts(c, [
            (FACTORY, "getToken(address)", ["address"], [exchange]),
            (exchange, "factoryAddress()", [], []),
            (exchange, "tokenAddress()", [], []),
            (token, "balanceOf(address)", ["address"], [exchange]),
            (MULTI, "getEthBalance(address)", ["address"], [exchange])])
        observed = [decode(["address"], v)[0] for v in facts[:3]]
        if observed != [token, FACTORY, token]:
            raise ValueError((exchange, observed))
        erc20, eth = [int.from_bytes(v, "big") for v in facts[3:]]
        markets[token] = dict(exchange=exchange, exchange_code=code(exchange), token_code=code(token), erc20=str(erc20), eth=str(eth))

    rows = []
    lanes = [(None, token) for token in tokens if token != WETH]
    lanes += [(DAI, STABLES[0]), (DAI, WETH), (STABLES[0], STABLES[1])]
    for left, right in lanes:
        for src, dst in ((left, right), (right, left)):
            for exact_in in (True, False):
                sized = src if exact_in else dst
                reserve = markets[sized]["erc20"] if sized else markets[src or dst]["eth"]
                for fraction in (100_000, 5_000):
                    amount = max(1, int(reserve) // fraction)
                    legs = [(src, "getTokenToEth"), (dst, "getEthToToken")]
                    if not exact_in:
                        legs.reverse()
                    calls = [dict(exchange=markets[coin]["exchange"],
                                  method=direction + ("Input" if exact_in else "Output") + "Price(uint256)")
                             for coin, direction in legs if coin]
                    rows.append(dict(source=src, target=dst, exact_in=exact_in, amount=str(amount), calls=calls))
    for leg in range(2):
        batch = []
        requests = []
        for row in rows:
            if len(row["calls"]) <= leg or (leg and "result" not in row["calls"][leg - 1]):
                continue
            call = row["calls"][leg]
            n = row["amount"] if leg == 0 else row["calls"][leg - 1]["result"]
            call["operand"] = n
            batch.append(call)
            requests.append((call["exchange"], call["method"], ["uint256"], [int(n)]))
        for start in range(0, len(batch), 4):
            group = list(zip(batch[start:start+4], requests[start:start+4], strict=True))
            payload = []
            for i, (_, (to, method, types, values)) in enumerate(group):
                data = keccak(text=method)[:4] + encode(types, values)
                payload.append(dict(id=i, jsonrpc="2.0", method="eth_call",
                                    params=[dict(to=to, data="0x"+data.hex(), gas="0x4c4b40"), c.selector]))
            response = c.session.post(endpoint, json=payload, timeout=25)
            response.raise_for_status()
            answers = {r["id"]: r for r in response.json()}
            if set(answers) != set(range(len(group))):
                raise ValueError("incomplete price batch")
            for i, (call, _) in enumerate(group):
                if "result" not in answers[i]:
                    error = answers[i].get("error", {})
                    msg = error.get("message", "").lower()
                    if not any(reason in msg for reason in ("invalid jump","invalidjump","revert")):
                        raise ValueError((call,error))
                    call["revert"] = error
                    continue
                raw = bytes.fromhex(answers[i]["result"][2:])
                if len(raw) not in (32, 4096):
                    raise ValueError((call, len(raw)))
                call["return_bytes"] = len(raw)
                call["result"] = str(int.from_bytes(raw[:32], "big"))
    return dict(block=snapshot["block"], selector=c.selector, factory=FACTORY, roles=roles, state_root=head["stateRoot"], wrapped=WETH, markets=markets, quotes=rows)


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("endpoint")
    cli.add_argument("output", type=Path)
    args = cli.parse_args()
    result = capture(args.endpoint)
    args.output.write_bytes((json.dumps(result, separators=(",", ":")) + "\n").encode("utf8"))
