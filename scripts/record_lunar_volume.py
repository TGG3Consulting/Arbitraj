"""Считает по оси USDC все SwapExecuted четырёх размещений LunarBase за контрольные 24 часа."""
import hashlib
import json
from collections import defaultdict

import requests
from eth_abi import decode
from eth_utils import keccak
from record_lunarbase import FOLDER, USDC, read, write


def capture():
    source=read(FOLDER/"lunarbase.json")
    window=read(FOLDER/"elfomo_volume_base.json")
    session=requests.Session()
    def rpc(method,params):
        if method not in ("eth_getBlockByNumber","eth_getLogs"):
            raise ValueError("Метод вне перечня чтения")
        r=session.post(source["endpoint"],json=dict(id=1,jsonrpc="2.0",method=method,params=params),timeout=45)
        r.raise_for_status()
        return r.json()["result"]
    def checked(h):
        b=rpc("eth_getBlockByNumber",[hex(h["number"]),False])
        if b["hash"]!=h["hash"] or int(b["timestamp"],16)!=h["timestamp"]:
            raise ValueError("Изменилась граница окна")
    first,last=window["start"]["number"],window["end"]["number"]
    for bound in ("previous","start","end"): checked(window[bound])
    if window["end"]["hash"]!=source["selector"]["blockHash"]:
        raise ValueError("Разные контрольные блоки")
    topic="0x"+keccak(text="SwapExecuted(address,bool,uint256,uint256,uint256)").hex()
    axes={}
    for m in source["markets"]:
        xs={x["signature"]:"0x"+x["response"]["result"][-40:] for x in m["views"] if x["signature"] in ("X()","Y()")}
        axes[m["address"]]=(xs["X()"],xs["Y()"])
    rows=defaultdict(lambda:dict(count=0,usdc_raw=0))
    pages,seen=[],set()
    for a in range(first,last+1,6000):
        z=min(last,a+5999)
        logs=rpc("eth_getLogs",[dict(fromBlock=hex(a),toBlock=hex(z),address=list(axes),topics=[topic])])
        pages.append(dict(first=a,last=z,count=len(logs),sha256=hashlib.sha256(
            json.dumps(logs,sort_keys=True,separators=(",",":")).encode()).hexdigest()))
        for log in logs:
            key=log["transactionHash"],log["logIndex"]
            if key in seen or log.get("removed") or not a<=int(log["blockNumber"],16)<=z or log["topics"]!=[topic]:
                raise ValueError("Событие не соответствует канонической странице")
            seen.add(key)
            pool=log["address"]
            x,y=axes[pool]
            _,xtoy,dx,dy,_=decode(["address","bool","uint256","uint256","uint256"],bytes.fromhex(log["data"][2:]))
            if USDC not in (x,y):raise ValueError("У пула нет оси USDC")
            row=rows[(pool,xtoy)]
            row["count"]+=1
            row["usdc_raw"]+=dx if x==USDC else dy
            row["example_tx"]=key[0]
    checked(window["end"])
    return dict(chain_id=8453,start=window["start"],end=window["end"],event=topic,axes=axes,pages=pages,
                count=len(seen),markets=[dict(pool=p,x_to_y=d,**rows[(p,d)]) for p in axes for d in (True,False)])


if __name__=="__main__":
    write(FOLDER/"lunar_volume_base.json",capture())
