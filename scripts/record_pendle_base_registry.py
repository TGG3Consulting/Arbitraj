"""Записывает все события двух официальных фабрик Base и физический доступ к PT/SY на исходном блоке."""
import argparse
import json
from pathlib import Path
from fork_fee_rpc import Reader, pool_blocks
from record_base_slipstream_executor import ROOT
from record_pendle_registry import ROUTER, TOPICS, read_many, snapshot

FACTORIES = {"marketFactoryV5":"0x59968008a703dc13e6beaeced644bdce4ee45d13",
             "marketFactoryV6":"0x81e80a50e56d10c501ff17b5fe2f662bd9ea4590"}
STATIC = "0xb4205a645c7e920bd8504181b1d7f2c5c955c3e7"
SOURCE = "https://github.com/pendle-finance/pendle-core-v2-public/blob/82e69f0b2279c85f250a87cb0fe16a2b432ebb72/deployments/8453-core.json"

def capture():
    head=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    reader=Reader("base",head); at=head["selector"]; last=head["block"]
    events=[]; ranges={}
    for name,addr in FACTORIES.items():
        count=int(reader.one("eth_getTransactionCount",[addr,at]),16)
        blocks=pool_blocks(reader,addr,0,last,0,count)
        found=reader.many(("eth_getLogs",[{"fromBlock":hex(h),"toBlock":hex(h),"address":addr,"topics":[TOPICS]}]) for h in blocks)
        for (h,incr),rows in zip(blocks.items(),found,strict=True):
            if any(int(r["blockNumber"],16)!=h or r.get("removed") for r in rows) or len(rows)>incr:
                raise ValueError("Событие фабрики не соответствует блоку создания")
            events.extend(rows)
        ranges[name]={"nonce":count,"changes":dict(sorted(blocks.items()))}
    captured=snapshot(reader.header,at,FACTORIES,ROUTER,STATIC,SOURCE,events,reader.one,lambda rows:read_many(rows,at,reader.one))
    captured["changes"]=ranges
    return captured

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("output",type=Path)
    ap.parse_args().output.write_bytes(json.dumps(capture(),separators=(",",":")).encode())

