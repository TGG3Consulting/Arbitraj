"""Оборот пар EtherVista за 24 часа до контрольного блока. Суммы взяты из событий Swap."""
import argparse
import json
from datetime import datetime, timezone
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_ethervista_wallets import NETWORKS

SWAP="0x"+keccak(text="Swap(address,uint256,uint256,uint256,uint256,address)").hex()


def capture(directory,endpoint):
    session=requests.Session()
    def read(method,params):
        res=session.post(endpoint,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=18)
        try:
            obj=res.json()
        except ValueError:
            res.raise_for_status()
            raise
        if "error" in obj:
            raise ValueError(f"{method}: {str(obj['error'])[:260]}")
        res.raise_for_status()
        if "result" not in obj:raise ValueError(str(obj.get("error"))[:240])
        return obj["result"]
    def header(h):
        data=read("eth_getBlockByNumber",[hex(h),False])
        if not data or int(data["number"],16)!=h:raise ValueError("неверная высота")
        return data
    height=directory["block"]
    end=header(height)
    if end["hash"]!=directory["selector"]["blockHash"] or end["stateRoot"]!=directory["state_root"]:
        raise ValueError("контрольный блок не совпадает с реестром")
    finish=int(end["timestamp"],16)
    lower=finish-86400
    distance=8192
    low=max(0,height-distance)
    while int(header(low)["timestamp"],16)>lower:
        if low==0:raise ValueError("до основания сети меньше суток")
        distance*=2
        low=max(0,height-distance)
    high=height
    while low<high:
        mid=(low+high)//2
        if int(header(mid)["timestamp"],16)<=lower:low=mid+1
        else:high=mid
    first=header(low)
    previous=header(low-1)
    if not int(previous["timestamp"],16)<=lower<int(first["timestamp"],16):
        raise ValueError("не доказана граница 24 часов")
    pools={row["pool"]:row["tokens"] for row in directory["registry"]}
    weth=directory["weth"]
    records={}
    def logs(a,b,addrs):
        try:
            found=read("eth_getLogs",[{"fromBlock":hex(a),"toBlock":hex(b),"address":addrs,"topics":[SWAP]}])
            if not isinstance(found,list):raise ValueError("неполный ответ фильтра")
        except (ValueError,requests.HTTPError) as exc:
            # Часть узлов ограничивает как диапазон высот, так и размер OR-списка адресов.
            msg=str(exc).lower()
            if not any(t in msg for t in ("limit","range","large","results","maximum","more than","size","413","exceeds")):raise
            if len(addrs)>1 and ("address" in msg or a==b):
                mid=len(addrs)//2
                return logs(a,b,addrs[:mid])+logs(a,b,addrs[mid:])
            if b>a:
                mid=(a+b)//2
                return logs(a,mid,addrs)+logs(mid+1,b,addrs)
            raise
        for log in found:
            if log["removed"] or log["topics"][0].lower()!=SWAP or log["address"].lower() not in addrs:
                raise ValueError("постороннее или удалённое событие")
            if not a<=int(log["blockNumber"],16)<=b or len(log["topics"])!=3:
                raise ValueError("нарушены границы ответа")
        return found

    addresses=sorted(pools)
    # Ответы непересекающихся прямоугольников (адреса, высоты) объединяются по ID лога.
    publicnode="publicnode" in endpoint
    group_size=10 if publicnode else 128
    span=100 if "drpc.org" in endpoint else 10000
    for i in range(0,len(addresses),group_size):
        group=addresses[i:i+group_size]
        for a in range(low,height+1,span):
            for log in logs(a,min(height,a+span-1),group):
                h=int(log["blockNumber"],16)
                idx=int(log["logIndex"],16)
                market=log["address"].lower()
                amounts=decode(["uint256"]*4,bytes.fromhex(log["data"][2:]))
                if (h,idx) in records:raise ValueError("дублированное событие")
                records[(h,idx)]=dict(pool=market,tx=log["transactionHash"],height=h,block_hash=log["blockHash"],
                                      log_index=idx,amounts=list(map(str,amounts)))
    by_block={}
    for (h,_),r in records.items():
        if by_block.setdefault(h,r["block_hash"])!=r["block_hash"]:raise ValueError("события разных ветвей")
    for h,v in by_block.items():
        if header(h)["hash"]!=v:raise ValueError("хеш события отсутствует на канонической ветви")
    if header(height)["hash"]!=end["hash"] or header(low)["hash"]!=first["hash"]:
        raise ValueError("выбранная история изменилась")
    paid=received=other=0
    for row in records.values():
        assets=pools[row["pool"]]
        if weth not in assets:
            other+=1
            continue
        j=assets.index(weth)
        paid+=int(row["amounts"][j])
        received+=int(row["amounts"][2+j])
    utc=lambda t:datetime.fromtimestamp(t,timezone.utc).isoformat()
    return dict(network=directory["network"],endpoint=endpoint,first_block=low,first_hash=first["hash"],
                last_block=height,last_hash=end["hash"],after_utc=utc(lower),through_utc=utc(finish),
                registered_pairs=len(pools),active_pairs=len({r["pool"] for r in records.values()}),
                transactions=len({r["tx"] for r in records.values()}),swaps=len(records),
                non_weth_swaps=other,weth_in_wei=str(paid),weth_out_wei=str(received),
                weth_turnover_wei=str(paid+received),events=[records[k] for k in sorted(records)])


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("directory",type=Path)
    ap.add_argument("output",type=Path)
    args=ap.parse_args()
    data=json.loads(args.directory.read_bytes())
    urls=NETWORKS[data["network"]][-1]
    if data["network"]=="ethereum":
        urls=sorted(urls,key=lambda url:"publicnode" not in url)
    for endpoint in urls:
        try:
            out=capture(data,endpoint)
            args.output.write_text(json.dumps(out,indent=2)+"\n",encoding="utf8")
            break
        except (requests.RequestException, ValueError, KeyError, TypeError) as error:
            print(endpoint,str(error)[:300],flush=True)
    else:
        raise ValueError("не удалось восстановить оборот из канонических событий Swap")
