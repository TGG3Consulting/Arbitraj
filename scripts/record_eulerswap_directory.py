"""Реестры EulerSwap V1/V2, оператор счёта LP и лимиты EVault на каноническом блоке."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_stabull_base import DATA, MC, encoded

V1 = ["address"] * 3 + ["uint112"] * 2 + ["uint256"] * 6 + ["address"]
STATIC = ["address"] * 6
DYNAMIC = ["uint112"] * 4 + ["uint80"] * 2 + ["uint64"] * 4 + ["uint40", "uint8", "address"]
ZERO = "0x" + "00" * 20


class Reader:
    def __init__(self, network):
        snapshot = json.loads((DATA / f"{network}.json").read_bytes())
        self.url = snapshot["endpoint"]
        self.tag = snapshot["selector"]
        self.height = snapshot["block"]
        self.http = requests.Session()
        head = self.rpc("eth_getBlockByNumber", [hex(self.height), False])
        assert head["hash"] == self.tag["blockHash"]

    def rpc(self, method, args):
        response = self.http.post(self.url, json={"jsonrpc": "2.0", "id": 1, "method": method, "params": args}, timeout=35)
        response.raise_for_status()
        reply = response.json()
        if "error" in reply:
            raise ValueError((method, reply["error"]))
        return reply["result"]

    def many(self, calls):
        rows = []
        for i in range(0, len(calls), 30):
            chunk = calls[i:i+30]
            payload = [(a, True, encoded(sig, args)) for a, sig, args, _ in chunk]
            data = keccak(text="aggregate3((address,bool,bytes)[])")[:4] + encode(["(address,bool,bytes)[]"], [payload])
            raw = self.rpc("eth_call", [{"to": MC, "data": "0x"+data.hex()}, self.tag])
            result = decode(["(bool,bytes)[]"], bytes.fromhex(raw[2:]))[0]
            assert len(result) == len(chunk)
            for (ok, body), (_, _, _, types) in zip(result, chunk):
                if not ok or not body:
                    rows.append(None)
                else:
                    values = decode(types.split(",") if isinstance(types, str) else types, body)
                    rows.append(values[0] if len(values) == 1 else list(values))
        return rows

    def code_hash(self, address):
        raw = bytes.fromhex(self.rpc("eth_getCode", [address, self.tag])[2:])
        assert raw
        return "0x"+keccak(raw).hex()


def capture(network, events):
    r = Reader(network)
    chain = int(r.rpc("eth_chainId", []), 16)
    assert chain == {"ethereum": 1, "base": 8453}[network]
    roles = json.loads((DATA/"eulerswap_roles.json").read_bytes())["networks"][str(chain)]
    for role in roles.values():
        assert r.code_hash(role["address"]) == role["runtime"]
    gens = []
    for version in (1, 2):
        factory = roles[f"V{version}Factory"]["address"]
        registry = factory if version == 1 else roles["V2Registry"]["address"]
        evc, implementation, count = r.many([
            (factory, "EVC()", (), "address"), (factory, "eulerSwapImpl()", (), "address"),
            (registry, "poolsLength()", (), "uint256")])
        assert implementation == roles[f"V{version}Implementation"]["address"]
        registered = []
        for start in range(0, count, 32):
            addresses, = r.many([(registry, "poolsSlice(uint256,uint256)", (start, min(start+32, count)), "address[]")])
            registered.extend(addresses)
        assert len(set(registered)) == count
        nonce = int(r.rpc("eth_getTransactionCount", [factory, r.tag]), 16)
        addresses = set(registered)
        history = []
        if version == 2 and events:
            from eulerswap_origins import from_receipts, origins
            candidate_file=DATA/f"eulerswap_{network}_origins.json.gz"
            history=(from_receipts(r,roles["V2Factory"],nonce,candidate_file) if candidate_file.exists()
                     else origins(r, roles["V2Factory"], nonce))
            addresses.update(log["pool"] for log in history)
            assert len(addresses) == nonce-1
        pools = []
        for address in sorted(addresses):
            reads = [(address,"getAssets()",(),"address,address"), (address,"getReserves()",(),"uint112,uint112,uint32")]
            if version == 1:
                reads.append((address, "getParams()", (), V1))
            else:
                reads.extend([(address,"getStaticParams()",(),STATIC),(address,"getDynamicParams()",(),DYNAMIC),
                              (factory,"deployedPools(address)",(address,),"bool"),
                              (roles["V2ProtocolFeeConfig"]["address"],"getProtocolFee(address)",(address,),"address,uint64")])
            tokens, reserves, config, *more = r.many(reads)
            assert tokens[0] < tokens[1] and config
            if version == 2:
                assert more[1]
            account = config[2] if version == 1 else config[4]
            checks = [(evc,"isAccountOperatorAuthorized(address,address)",(account,address),"bool"),
                      (registry,"poolByEulerAccount(address)",(account,),"address")]
            for token in tokens:
                checks.append((token,"decimals()",(),"uint8"))
            for i in range(2):
                checks.append((address,"getLimits(address,address)",(tokens[i],tokens[1-i]),"uint256,uint256"))
            installed, listed, d0, d1, *limits = r.many(checks)
            if version==1:
                assert listed==address
            row = dict(address=address,tokens=tokens,reserves=reserves,config=config,account=account,
                       installed=installed,registered=address in registered,by_account=listed,decimals=[d0,d1],limits=limits)
            if history:
                origin=next(x for x in history if x["pool"]==address)
                assert origin["tokens"]==tokens and origin["config"]==config
            if more:
                row.update(dynamic=more[0],protocol_fee=more[2])
            quotes = []
            for i, lim in enumerate(limits):
                if lim and min(lim)>0:
                    for exact, side in [(True,i),(False,1-i)]:
                        bound = lim[0 if exact else 1]
                        for amount in sorted({min(bound, max(1, 10**[d0,d1][side] // 1000)),min(bound, 10**[d0,d1][side])}):
                            quotes.append((i,exact,amount))
            values = r.many([(address,"computeQuote(address,address,uint256,bool)",(tokens[i],tokens[1-i],amount,exact),"uint256")
                             for i,exact,amount in quotes])
            row["quotes"] = [dict(side=i,exact_in=exact,amount=amount,result=v) for (i,exact,amount),v in zip(quotes,values)]
            pools.append(row)
        gens.append(dict(version=version, factory=factory, implementation=implementation, evc=evc,
                         registry=registry, registered=count, nonce=nonce, deployments=history, pools=pools))
    return dict(chain=chain, block=r.height, selector=r.tag, roles=roles, generations=gens)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("network", choices=["base", "ethereum"])
    ap.add_argument("output", type=Path)
    ap.add_argument("--events", action="store_true")
    args = ap.parse_args()
    args.output.write_text(json.dumps(capture(args.network,args.events), separators=(",",":")))

