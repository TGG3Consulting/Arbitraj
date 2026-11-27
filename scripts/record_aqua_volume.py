"""Объём входов в нативных единицах токенов по алгоритму DefiLlama helpers/oneinchAqua."""
import argparse
import gzip
import json
from collections import Counter
from pathlib import Path

from aqua_evidence import EARLY, REGISTRY, ROUTER, TOPICS, unpack
from record_aqua_directory import NODES, one
from record_base_slipstream_executor import ROOT


def account(rows):
    position = {(r["registry"], r["tx"], r["index"]): r for r in rows if r["event"] == "Pushed"}
    registrations = set()
    for r in rows:
        if r["event"] != "Shipped":
            continue
        index = r["index"] + 1
        while (p := position.get((r["registry"], r["tx"], index))) and all(
                p[k] == r[k] for k in ("maker", "app", "order")):
            registrations.add((r["registry"], r["tx"], index))
            index += 1
    fills = [r for key, r in position.items() if key not in registrations]
    group = lambda r: (r["registry"], r["tx"], r["maker"], r["token"])
    pushed, pulled, legs = Counter(), Counter(), set()
    for r in fills:
        pushed[group(r)] += r["amount"]
        legs.add((*group(r), r["order"]))
    for r in rows:
        if r["event"] == "Pulled" and group(r) in pushed and (*group(r), r["order"]) not in legs:
            pulled[group(r)] += r["amount"]
    gross, net, app_net = Counter(), Counter(), Counter()
    for r in fills:
        total = pushed[group(r)]
        credit = r["amount"] * max(total - pulled[group(r)], 0) // total if total else 0
        gross[r["token"]] += r["amount"]
        net[r["token"]] += credit
        if r["app"] == ROUTER:
            app_net[r["token"]] += credit
    return {"registration_pushes":len(registrations), "fill_pushes":len(fills),
            "gross_raw":dict(sorted(gross.items())), "net_raw":dict(sorted(net.items())),
            "swapvm_net_raw":dict(sorted(app_net.items()))}


def capture(network):
    source = ROOT / "tests/data/first_run_dex_2026-11-26" / f"aqua_{network}_day.json.gz"
    with gzip.open(source) as f:
        day = json.load(f)
    low, high = day["fromBlock"], day["through"]
    def rpc(method, params):
        for url in NODES[network]:
            try:
                return one(url, method, params)
            except (ValueError, OSError) as e:
                last = e
        raise RuntimeError(method) from last
    def header():
        if rpc("eth_getBlockByNumber", [hex(high),False])["hash"] != day["selector"]["blockHash"]:
            raise ValueError("canonical header")
    header()
    topics = [h for h,n in TOPICS.items() if n in ("Shipped", "Pushed", "Pulled")]
    rows = []
    # The historical and production registry are the two physical addresses of the source scope.
    for start in range(low, high+1, 9000):
        logs = rpc("eth_getLogs", [{"address":[EARLY,REGISTRY],"topics":[topics],
            "fromBlock":hex(start),"toBlock":hex(min(high,start+8999))}])
        for log in logs:
            name, values = unpack(log)
            maker, app, order = values[:3]
            row = dict(registry=log["address"].lower(),tx=log["transactionHash"],blockHash=log["blockHash"],
                       index=int(log["logIndex"],16), event=name,maker=maker,app=app,order=order)
            if name != "Shipped":
                row.update(token=values[3],amount=values[4])
            rows.append(row)
    header()
    assert Counter((r["registry"],r["event"]) for r in rows) == {
        (a,n):v for a,n,v in day["counts"] if a in (EARLY,REGISTRY) and n in ("Shipped","Pushed","Pulled")}
    return dict(network=network,selector=day["selector"],fromBlock=low,through=high,
                source="https://github.com/DefiLlama/dimension-adapters/blob/be9967c3/helpers/oneinchAqua.ts",
                **account(rows), records=rows)


if __name__ == "__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("network",choices=NODES)
    cli.add_argument("output",type=Path)
    a=cli.parse_args()
    a.output.write_text(json.dumps(capture(a.network),separators=(",",":")),encoding="utf8")
