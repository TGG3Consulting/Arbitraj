"""Read-only settlement replay for pre-held IBT identified in eight untested Base Spectra markets."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from record_omni_v2_base import ENDPOINTS
from record_spectra_wallets import wallet
from record_base_slipstream_executor import ROOT, rpc


def read_archive(path):
    with gzip.open(path) as file:
        return json.load(file)


def replay(output):
    folder = ROOT / "tests/data/first_run_dex_2026-11-26"
    snapshot = read_archive(folder / "spectra_base_directory.json.gz")
    holders = json.loads((folder / "spectra_extra_ibt_holders.json").read_bytes())
    if holders["selector"] != snapshot["selector"]:
        raise ValueError("holders and directory use different parents")
    directory = {market["pool"]:market for market in snapshot["markets"]}
    rows = []
    endpoint_index = 0
    checked = set()
    for pool in holders["markets"]:
        market = directory[pool["pool"]]
        if pool["token"] != market["coin0"]:
            raise ValueError("different IBT at expected pool")
        source = max(pool["holders"],key=lambda candidate:int(candidate["balance"]))
        cases = []
        for denominator in (10000,1000):
            budget = max(10**market["decimals0"]//denominator,1)
            for mode in ("v5","pool","router"):
                # Continue an interrupted read at the same individual sample. Earlier cases remain in memory.
                for attempt in range(len(ENDPOINTS)):
                    endpoint = ENDPOINTS[endpoint_index]
                    try:
                        if endpoint not in checked:
                            header = rpc(endpoint,"eth_getBlockByHash",[snapshot["selector"]["blockHash"],False])
                            if header["stateRoot"] != snapshot["state_root"]:
                                raise ValueError("unexpected canonical root")
                            checked.add(endpoint)
                        result = wallet(endpoint,snapshot,market,budget,mode,("owned_ibt",source["owner"]))
                    except (requests.RequestException,ValueError) as error:
                        if attempt+1 == len(ENDPOINTS):
                            raise
                        print(f"{endpoint} {pool['pool']} {mode}: {str(error)[:160]}",flush=True)
                        endpoint_index = (endpoint_index+1) % len(ENDPOINTS)
                    else:
                        funding = result.get("funding")
                        if not funding or int(funding["results"][0]["returnData"],16) != int(source["balance"]):
                            raise ValueError("archived balance was not reproduced")
                        result["endpoint"]=endpoint
                        cases.append(result)
                        break
        rows.append({"pool":market["pool"],"template":market["template"],"kind":market["kind"],"maturity":market["maturity"],"wallets":cases})
    output.write_text(json.dumps(dict(selector=snapshot["selector"],state_root=snapshot["state_root"],markets=rows),separators=(",",":")),encoding="utf8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    replay(parser.parse_args().output)

