"""Отдельная, автономная проверка полноты регистраций и чувствительности к удалению реального тарифа."""
from copy import deepcopy
import json
from pathlib import Path
import sys

from record_fork_fee_registry import FIXTURES, ROOT, SPECS, verify


def check(snapshot):
    network, independent = {1:("ethereum","community_v3_storage"),8453:("base","derpdex_directory")}[snapshot["chain_id"]]
    reference = json.loads((FIXTURES/(independent+".json")).read_bytes())
    if reference["selector"] != snapshot["selector"] or (reference.get("block") or reference["to"]) != snapshot["block"]:
        raise ValueError("different archival head")
    config = json.loads((ROOT/"config/first_run_dex.json").read_bytes())
    targets = {p["id"]:p["settings"]["dex.factory"].lower() for p in config if p["network"]==network and p["id"] in SPECS}
    if {r["id"]:r["factory"] for r in snapshot["factories"]} != targets or len(snapshot["factories"])!=2:
        raise ValueError("incorrect network or factory selection")
    for row in snapshot["factories"]:
        if row["state_root"] != reference["state_root"] or row["kind"] != SPECS[row["id"]][0]:
            raise ValueError("factory proof bound to the wrong state root or ABI")
        verify(row)
        tiers={int(f):sp for f,sp in row["tiers"].items()}
        activations={v["fee"]:v["spacing"] for v in row["activations"]}
        if not tiers or activations!=tiers or len(activations)!=len(row["activations"]):
            raise ValueError("registry lacks activation boundaries")
        for a in row["activations"]:
            if a["before"]!=0 or not row["deployment"]<=a["block"]<=snapshot["block"] or a["parent"]==a["hash"]:
                raise ValueError("invalid activation interval")
            if ("tx" in a)!=(row["kind"]=="kyber"):
                raise ValueError("incorrect registration ABI")
        used={p["fee"] for p in row["markets"]}
        omitted=next((f for f in tiers if f not in used),next(iter(tiers)))
        altered=deepcopy(row)
        del altered["tiers"][str(omitted)]
        try:
            verify(altered)
        except (ValueError,KeyError):
            continue
        raise ValueError("missing registry entry accepted")


if __name__=="__main__":
    check(json.loads(Path(sys.argv[1]).read_bytes()))
