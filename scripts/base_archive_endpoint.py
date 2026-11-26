"""Выбирает публичный узел Base для чтения по архивному каноническому хешу; адрес запроса сохраняется в результате."""
import requests

from record_base_slipstream_executor import rpc


def available(source):
    tag = source["selector"]
    failures = []
    for url in dict.fromkeys(("https://mainnet.base.org", source["endpoint"])):
        try:
            if int(rpc(url, "eth_chainId", []), 16) != 8453:
                raise ValueError("Узел другой сети")
            stage, = rpc(url, "eth_simulateV1", [{"blockStateCalls":[{"calls":[{
                "to":"0x4200000000000000000000000000000000000006", "data":"0x18160ddd"}]}],
                "validation":False}, tag])
            if stage["parentHash"] != tag["blockHash"] or int(stage["calls"][0]["returnData"],16)==0:
                raise ValueError("Узел не вернул ожидаемое архивное состояние")
            return {**source, "endpoint":url}
        except (requests.RequestException, ValueError, KeyError, IndexError) as error:
            failures.append(f"{url}: {error}")
    raise RuntimeError("Нет узла с каноническим состоянием Base: "+"; ".join(failures))
