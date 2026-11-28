"""Verify previously indexed Base Raindex V5 orders with spendable output vaults on the recorded block."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from record_raindex_wallets import ROOT, capture


def books_for_replay():
    path = ROOT / "tests/data/first_run_dex_2026-11-26/raindex_base_legacy_directory.json.gz"
    with gzip.open(path) as file:
        snapshot=json.load(file)
    book=next(row for row in snapshot["books"] if row["version"]==5)
    live=[]
    for digest, state in book["states"].items():
        legs=[dict(quote,receive=out["token"],j=out["j"]) for out in state.get("outputs",[])
              for quote in out.get("legs",[]) if quote.get("positive")]
        if legs:
            live.append({"hash":digest,"order":state["order"],"legs":legs})
    if not live:
        raise ValueError("no confirmed Raindex V5 quote")
    return [{"version":book["version"],"book":book["book"],"runtime":book["runtime"],"live":live}]


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("compiled",type=Path)
    p.add_argument("output",type=Path)
    args=p.parse_args()
    compiled=json.loads(args.compiled.read_bytes())
    books=books_for_replay()
    for endpoint in ("https://base.gateway.tenderly.co","https://mainnet.base.org"):
        try:
            result=capture("base",endpoint,compiled,selected_books=books)
            if not any(case["by_output"]["stage"]=="paid" for pair in result["pairs"] for case in pair["cases"]):
                direct=capture("base",endpoint,None,selected_books=books)
                result["direct"]=direct["pairs"]
        except (ValueError,KeyError,requests.RequestException) as error:
            print(endpoint,str(error)[:160],flush=True)
            if endpoint=="https://mainnet.base.org":
                raise
        else:
            result["endpoint"]=endpoint
            args.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
            break
