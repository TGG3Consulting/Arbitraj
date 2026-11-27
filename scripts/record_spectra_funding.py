"""Ищет реальных плательщиков по Transfer IBT и underlying. Кандидатов подтверждает баланс, код и газ на выбранном H."""
import gzip
import json
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor

import requests
from eth_utils import keccak
from record_omni_flux_events import ENDPOINT
from record_spectra_base import AT, BLOCK, call, many, read
from record_woo_base import HOLDERS

DATA=Path("tests/data/first_run_dex_2026-11-26")
TOPIC="0x"+keccak(text="Transfer(address,address,uint256)").hex()


def anchors(token, end, span):
    p={"address":token,"fromBlock":hex(end-span+1),"toBlock":hex(end),"topics":[TOPIC]}
    req=requests.post(ENDPOINT,json={"jsonrpc":"2.0","id":1,"method":"eth_getLogs","params":[p]},timeout=20)
    req.raise_for_status()
    j=req.json()
    if "result" not in j:
        return []
    out=[]
    for log in j["result"][-90:]:
        if len(log["topics"])==3:
            out.extend(("0x"+topic[-40:],log["transactionHash"]) for topic in log["topics"][1:])
    return out


def capture():
    directory=json.loads(gzip.decompress((DATA/"spectra_base_directory.json.gz").read_bytes()))
    pools=directory["markets"]
    blocked={r["pool"] for r in pools}|{r["coin1"] for r in pools}|{r["coin0"] for r in pools}|{"0x"+"00"*20}
    tokens=sorted({r[k] for r in pools for k in ("underlying","coin0")} - set(HOLDERS))
    def holder(token):
        known={}
        errors=[]
        for end,span in ((BLOCK,10000),(BLOCK,100000),(BLOCK-100000,100000),(BLOCK-1000000,100000),(BLOCK-10000000,100000)):
            try:
                found=anchors(token,end,span)
            except (requests.RequestException,ValueError) as error:
                errors.append({"to":end,"span":span,"error":str(error)[:180]})
                continue
            for a,tx in found:
                if a not in blocked:known[a]=tx
            if len(known)>=12:
                break
        if not known:
            return dict(token=token,examined=0,window_errors=errors)
        balances=many([call(token,"balanceOf(address)","uint256",a) for a in known],optional=True)
        for b,a in sorted(((b,a) for a,b in zip(known,balances) if b),reverse=True):
            if read("eth_getCode",[a,AT])!="0x":
                continue
            native=read("eth_getBalance",[a,AT])
            if int(native,16):
                return dict(token=token,owner=a,balance=str(b),code="0x",native=native,transfer=known[a],examined=len(known))
        return dict(token=token,examined=len(known),window_errors=errors)
    with ThreadPoolExecutor(max_workers=4) as executor:
        rows=list(executor.map(holder,tokens))
    return dict(selector=AT,endpoint=ENDPOINT,holders=rows)


if __name__=="__main__":
    (DATA/"spectra_base_funding.json").write_text(json.dumps(capture(),indent=2)+"\n",encoding="utf8")
