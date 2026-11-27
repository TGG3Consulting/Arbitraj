"""Обычный покупатель на состоянии и времени исходного Base-блока, без повторов чужих обновлений."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import decode, encode
from record_e3_base import OWNER, UNI, WETH, USDC
from record_eulerswap_directory import DATA, Reader
from record_kaliber_access import AAPL, VAULTS, WITNESS


def capture(bytecode):
    r=Reader("base")
    source=json.loads(gzip.decompress((DATA/"kaliber_base_vaults.json.gz").read_bytes()))
    historic,=[w for w in source["windows"] if w["name"]=="2026-10-08"]
    pair,={e["topics"][1] for e in historic["events"]}
    heads=[r.tag["blockHash"],r.rpc("eth_getTransactionReceipt",[WITNESS])["blockHash"]]
    for w in historic["receipts"].values():
        if len(heads)==6:break
        if w["block"] not in heads:heads.append(w["block"])
    types="(address,address,address,address,address,bytes32,uint256,uint256)"
    def observation(vault,amount,minimum,tag):
        order=(UNI,WETH,USDC,AAPL,vault,bytes.fromhex(pair[2:]),amount,minimum)
        data="0x"+bytecode+encode([types],[order]).hex()
        tx={"from":OWNER,"gas":hex(4000000),"value":hex(10**18),"data":data}
        payload=r.rpc("eth_call",[tx,tag,{OWNER:{"balance":hex(2*10**18)}}])
        addr,num,stamp,ok,returned,funding,spent,net=decode(
            ["address","uint256","uint256","bool","bytes","uint256","uint256","uint256"],bytes.fromhex(payload[2:]))
        assert funding>=amount
        if ok:assert spent==amount and net>=minimum
        else:assert spent==net==0
        return dict(payer=addr,block=num,timestamp=stamp,source=USDC,target=AAPL,cash=vault,quantity=amount,
                    minimum=minimum,ok=ok,response="0x"+returned.hex(),funding=funding,spent=spent,received=net)
    results=[]
    for h in heads:
        tag={"blockHash":h,"requireCanonical":True}
        block=r.rpc("eth_getBlockByHash",[h,False])
        for v in VAULTS:
            for amount in (10**6,500250000):
                row=observation(v,amount,1,tag)
                assert row["block"]==int(block["number"],16) and row["timestamp"]==int(block["timestamp"],16)
                if row["ok"]:
                    strict=observation(v,amount,row["received"]+1,tag)
                    assert not strict["ok"]
                    row["strict"]=strict
                results.append(dict(tag=tag,**row))
    return dict(tag=r.tag,origin=OWNER,witness=WITNESS,pair=pair,results=results)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("binaries",type=Path);ap.add_argument("output",type=Path)
    args=ap.parse_args();binfile,=args.binaries.glob("*_KaliberCashProbe.bin")
    args.output.write_text(json.dumps(capture(binfile.read_text().strip()),separators=(",",":")),encoding="utf8")

