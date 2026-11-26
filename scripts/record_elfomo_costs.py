"""Читает архивную цену размещения/обмена ElfomoFi Base для конкретного артефакта GitHub Actions."""
import sys
from pathlib import Path

import requests
from eth_utils import keccak

from record_base_slipstream_executor import calldata
from record_elfomo_executor import FOLDER, ORDER, read, write


def capture(compiled, run_url, combined=False):
    files = {"elfomo":"elfomo_executor_base"} if not combined else {
        "elfomo":"base_pmm_executor_elfomo", "lunarbase":"base_pmm_executor_lunar"}
    archives = {k:read(FOLDER / (v+".json")) for k,v in files.items()}
    replay = archives["elfomo"]
    volumes = [read(FOLDER / "elfomo_volume_base.json")]
    if combined:
        volumes.append(read(FOLDER / "lunar_volume_base.json"))
    runtime = "0x" + keccak(bytes.fromhex(compiled["runtime"])).hex()
    if any(runtime != a["runtime_keccak"] or a["selector"] != replay["selector"] for a in archives.values()):
        raise ValueError("Стоимость и архивная приёмка относятся к разному коду")
    session = requests.Session()
    tag = replay["selector"]

    def rpc(method, params):
        if method not in ("eth_estimateGas", "eth_call", "eth_getTransactionReceipt"):
            raise ValueError("Запрос вне перечня чтения")
        resp = session.post(replay["endpoint"], json=dict(jsonrpc="2.0", id=1, method=method, params=params), timeout=30)
        resp.raise_for_status()
        data = resp.json()
        if "error" in data:
            raise ValueError(data["error"])
        return data

    case = replay["cases"][0]
    create = rpc("eth_estimateGas", [{"from": case["owner"], "data": "0x" + compiled["creation"]}, tag])
    creation_size = len(bytes.fromhex(compiled["creation"]))
    selector = "0x" + keccak(text=ORDER)[:4].hex()
    swap_size = len(bytes.fromhex(next(c["data"][2:] for c in case["calls"] if c["data"].startswith(selector))))
    gas_by_market = {market:[c["gas"] for c in a["cases"]] for market,a in archives.items()}
    gas = [v for market in gas_by_market.values() for v in market]
    receipts = []
    for pair in (p for v in volumes for p in v["markets"] if p.get("example_tx")):
        receipt = rpc("eth_getTransactionReceipt", [pair["example_tx"]])["result"]
        receipts.append({k: receipt.get(k) for k in ("transactionHash", "blockHash", "gasUsed", "effectiveGasPrice",
                                                     "l1Fee", "operatorFeeScalar", "operatorFeeConstant")})
    oracle = {}
    for name, size, used in (("swap", swap_size, max(gas)), ("creation", creation_size, int(create["result"], 16))):
        def ask(method, arg):
            data = calldata(method, ["uint256"], [arg])
            return rpc("eth_call", [{"to": "0x420000000000000000000000000000000000000f", "data": data}, tag])
        oracle[name] = dict(unsigned_envelope_bytes_bound=size + 128, calldata_bytes=size,
                            fee_response=ask("getL1FeeUpperBound(uint256)", size + 128),
                            operator_gas=used, operator_fee_response=ask("getOperatorFee(uint256)", used))
    info = dict(run=run_url, selector=tag, compile_runtime=runtime, creation_bytes=creation_size,
                runtime_bytes=len(bytes.fromhex(compiled["runtime"])), creation_sender=case["owner"], creation_gas=create,
                swaps_gas=gas, fee_receipt_examples=receipts, fee_oracle=oracle)
    if combined:
        info["gas_by_market"] = gas_by_market
    return info


if __name__ == "__main__":
    combined = "--base-pmm" in sys.argv[3:]
    target = "base_pmm_deployment_estimate.json" if combined else "elfomo_deployment_estimate_base.json"
    write(FOLDER / target, capture(read(Path(sys.argv[1])), sys.argv[2], combined))
