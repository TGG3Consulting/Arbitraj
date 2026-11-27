"""Ставки Topaz от архивного плательщика, возраст и активные настройки модуля, канонические события скидок."""
import gzip
import json
from concurrent.futures import ThreadPoolExecutor
from eth_abi import decode,encode
from eth_utils import keccak
from record_fynd_markets import DATA,NODES,rpc
from record_spoke_markets import data

ZERO="0x"+"00"*20

def collect(chain):
    d=json.loads(gzip.decompress((DATA/f"topaz_v1_{chain}_directory.json.gz").read_bytes()))
    pay=json.loads(gzip.decompress((DATA/f"topaz_v1_{chain}_wallets.json.gz").read_bytes()))
    url=NODES[chain];m=d["roles"]["DynamicSwapFeeModule"]["address"];last=d["height"]
    names=("DiscountedRegistered(address,uint24)","DiscountedDeregistered(address)")
    if chain=="base":
        url="https://base.gateway.tenderly.co"
        from record_fynd_markets import events
        lo,hi=0,last
        while lo<hi:
            mid=(lo+hi)//2
            if rpc(url,"eth_getCode",[m,hex(mid)])=="0x":lo=mid+1
            else:hi=mid
        first=lo
        if rpc(url,"eth_getCode",[m,hex(first-1)])!="0x":raise ValueError("недоказанное начало истории")
        history=[row for name in names for row in events(chain,m,name,first,last)]
    else:
        first=0
        topics=["0x"+keccak(text=s).hex() for s in names]
        history=rpc(url,"eth_getLogs",[dict(address=m,topics=[topics],fromBlock="0x0",toBlock=hex(last))])
    owners={v["payer"].lower() for v in pay["samples"]}|{ZERO}
    owners.update("0x"+log["topics"][1][-40:] for log in history)
    for log in history:
        header=rpc(url,"eth_getBlockByNumber",[log["blockNumber"],False])
        if header["hash"]!=log["blockHash"] or log["address"].lower()!=m or log.get("removed"):raise ValueError("не подтверждена регистрация скидки")
    def frame(height):
        h=rpc(url,"eth_getBlockByNumber",[hex(height),False]);at=dict(blockHash=h["hash"],requireCanonical=True)
        def view(addr,sig,argtypes=(),args=(),ret=("uint256",),sender=ZERO):
            call=dict(to=addr,data="0x"+data(sig,argtypes,args).hex(),**{"from":sender})
            raw=rpc(url,"eth_call",[call,at]);return dict(call=call,raw=raw,value=list(decode(ret,bytes.fromhex(raw[2:]))))
        def market(r):
            p=r["pool"]
            return dict(pool=p,config=view(m,"dynamicFeeConfig(address)",["address"],[p],["uint24","uint24","uint64"]),
                    fees={a:view(p,"fee()",sender=a) for a in sorted(owners)})
        with ThreadPoolExecutor(max_workers=4) as e:markets=list(e.map(market,d["markets"]))
        return dict(height=height,selector=at,timestamp=int(h["timestamp"],16),state_root=h["stateRoot"],
                    defaults={k:view(m,k+"()") for k in ("defaultFeeCap","defaultScalingFactor","secondsAgo","SCALING_PRECISION")},
                    discounts={a:view(m,"discounted(address)",["address"],[a]) for a in sorted(owners)},markets=markets)
    blocks=[frame(n) for n in (last-1,last,last+1)]
    if blocks[1]["selector"]!=d["selector"] or blocks[1]["state_root"]!=d["state_root"]:raise ValueError("другой снимок")
    return dict(chain=chain,module=m,from_block=first,to_block=last,registrations=history,blocks=blocks)

if __name__=="__main__":
    import argparse
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("network",choices=["base","ethereum"]);net=p.parse_args().network
    (DATA/f"topaz_{net}_fees.json.gz").write_bytes(gzip.compress(json.dumps(collect(net),separators=(",",":")).encode(),mtime=0))
