"""Подбирает реального держателя из предшествующих Transfer и сверяет код/баланс на контрольном хеше."""
import argparse
import json
from pathlib import Path
from eth_utils import keccak
from record_base_launch_bindings import read

def record(d):
    at,height=d["selector"],d["height"]
    tokens={r["terms"]["quote"] for r in d["rows"] if r["group"]=="o1-launchpad" and "terms" in r} - {"0x"+"00"*20}
    out={}
    for token in sorted(tokens):
        seen=set()
        for width in (50,500,1000):
            logs=read("eth_getLogs",[dict(address=token,fromBlock=hex(height-width),toBlock=hex(height),topics=["0x"+keccak(text="Transfer(address,address,uint256)").hex()])])
            for log in reversed(logs):
                for t in (log["topics"][2],log["topics"][1]):
                    wallet="0x"+t[-40:]
                    if wallet in seen:continue
                    seen.add(wallet)
                    if int(wallet,16)<65536 or read("eth_getCode",[wallet,at])!="0x":continue
                    call="0x70a08231"+wallet[2:].zfill(64)
                    amount=read("eth_call",[dict(to=token,data=call),at])
                    if int(amount,16)>=10000:
                        out[token]=dict(payer=wallet,quantity=str(int(amount,16)),transfer=log,balance_call=call,balance_result=amount)
                        break
                if token in out:break
            if token in out:break
    return dict(selector=at,tokens=out)

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory",type=Path);parser.add_argument("output",type=Path);a=parser.parse_args()
    a.output.write_text(json.dumps(record(json.loads(a.directory.read_bytes())),indent=2)+"\n",encoding="utf8")
