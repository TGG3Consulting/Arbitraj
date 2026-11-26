"""Refresh only inaccessible pool pages, and capture network-specific DEX volume for prioritization."""
import argparse
import json
import time
from datetime import datetime, timezone
from pathlib import Path

import requests


def now():
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def write(path, data):
    tmp = path.with_suffix(".part")
    tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    tmp.replace(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sources", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    result = json.loads(args.output.read_bytes()) if args.output.exists() else {}
    with requests.Session() as client:
        for network, chain in [("base", "Base"), ("ethereum", "Ethereum")]:
            dst = result.setdefault(network, {})
            table = dst.setdefault("pages", {})
            url = "https://api.llama.fi/overview/dexs/" + chain
            response = client.get(url, params={"excludeTotalDataChart":"true","excludeTotalDataChartBreakdown":"true","dataType":"dailyVolume"}, timeout=20)
            response.raise_for_status()
            dst["volume"] = {"at": now(), "url": response.url, "by_id": {
                str(r["defillamaId"]): r["total24h"] for r in response.json()["protocols"]
                if r.get("defillamaId") is not None and r.get("total24h") is not None
            }}
            originals = args.sources / f"{network}-integration-2026-11-26/sources/sample-pools.json"
            for dex, old in json.loads(originals.read_bytes()).items():
                if old.get("state") == "enumerated" or table.get(dex, {}).get("state") == "enumerated":
                    continue
                url = old["url"]
                page = {"at":now(), "url":url}
                try:
                    response = client.get(url, timeout=15)
                    page["status"] = response.status_code
                    if response.status_code == 200:
                        data = response.json()["data"]
                        samples = [{"id": r["id"], "name": r["attributes"]["name"], "address": r["attributes"]["address"],
                                    "dex":r["relationships"]["dex"]["data"]["id"]} for r in data]
                        if any(r["dex"] != dex for r in samples):
                            raise ValueError("unexpected DEX")
                        page.update(state="enumerated", rows_page1=len(samples), samples=samples)
                    else:
                        page["state"] = "http"
                except (requests.RequestException, KeyError, ValueError) as error:
                    page.update(state="error", detail=type(error).__name__)
                table[dex] = page
                write(args.output, result)
                if page.get("status") == 429:
                    try:
                        wait = int(response.headers.get("Retry-After", "12"))
                    except ValueError:
                        wait = 30
                    time.sleep(max(3, wait))
                else:
                    time.sleep(2.5)
            write(args.output, result)
            print(network, len(table), sum(x["state"]=="enumerated" for x in table.values()), flush=True)


if __name__ == "__main__":
    main()
