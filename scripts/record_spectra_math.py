"""Записывает ABI штатных загрузчиков и get_dy для всех 88 пулов Spectra на одном состоянии."""
import gzip
import json
import sys
from pathlib import Path

from eth_abi import decode
from record_omni_flux_registry import batch, data
from record_spectra_base import AT, call

DATA = Path("tests/data/first_run_dex_2026-11-26")
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/"tools/arbitrum_record"))
from curve_crypto_pools import bot_calls


def capture():
    directory=json.loads(gzip.decompress((DATA/"spectra_base_directory.json.gz").read_bytes()))
    records=[]
    queue=[]
    for market in directory["markets"]:
        p=market["pool"]
        stable=market["kind"]=="sng"
        r=dict(pool=p,template=market["template"],kind=market["kind"],decimals=[market["decimals0"],market["decimals1"]],
               balances=[str(market[k]) for k in ("balance0","balance1")],calls={},quotes=[])
        records.append(r)
        if stable:
            methods=[(s,[],[]) for s in ("A_precise()","fee()","offpeg_fee_multiplier()","stored_rates()")]
        else:
            methods=[(sig,["uint256"] if arg else [],[int(arg,16)] if arg else []) for sig,arg in bot_calls(2,True)]
        for sig,types,args in methods:
            payload=data(sig,types,args)
            queue.append((r,p,payload,sig if stable else "0x"+payload.hex(),stable))
        for key,value in market.items():
            if key.startswith("quote_"):
                _,i,amount=key.split("_")
                r["quotes"].append(dict(i=int(i),dx=amount,dy=str(value) if value is not None else None))
    for start in range(0,len(queue),60):
        part=queue[start:start+60]
        for (r,_,_,key,stable),(ok,raw) in zip(part,batch(AT,[(p,payload) for _,p,payload,_,_ in part]),strict=True):
            if stable:
                if key=="stored_rates()":
                    r["calls"][key]=[str(x) for x in decode(["uint256[]"],raw)[0]] if ok else None
                else:
                    r["calls"][key]=str(int.from_bytes(raw,"big")) if ok else None
            else:
                r["calls"][key]={"result":"0x"+raw.hex()} if ok else {"error":"reverted"}
    return dict(selector=AT,timestamp=directory["timestamp"],markets=records)


if __name__=="__main__":
    (DATA/"spectra_base_math.json.gz").write_bytes(gzip.compress(json.dumps(capture(),separators=(",",":")).encode(),mtime=0))
