"""Закрепляет семантику переводов Ryze. Только чтение Base через WebSocket и GET первичного исходника."""
import gzip
import json
from pathlib import Path

import requests
import websocket
from eth_utils import keccak

ROOT=Path(__file__).resolve().parents[1]; DATA=ROOT/"tests/data/first_run_dex_2026-11-26"
SLOTS={x:"0x"+keccak(text="org.zeppelinos.proxy."+x).hex() for x in ("implementation","admin")}
TOKENS={"WETH":"0x4200000000000000000000000000000000000006","USDC":"0x833589fcd6edb6e08f4c7c32d4f71b54bda02913","cbBTC":"0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf"}

def collect():
    directory=json.loads((DATA/"base.json").read_bytes());tag=directory["selector"]
    ws=websocket.create_connection("wss://base-rpc.publicnode.com",timeout=16,suppress_origin=True)
    def rpc(method,params):
        if method not in ("eth_getCode","eth_getStorageAt","eth_getBlockByNumber","eth_chainId","eth_call"):raise ValueError(method)
        ws.send(json.dumps(dict(id=1,jsonrpc="2.0",method=method,params=params)));r=json.loads(ws.recv())
        if "result" not in r:raise RuntimeError(str(r.get("error"))[:180])
        return r["result"]
    def contract(addr):
        code=rpc("eth_getCode",[addr,tag])
        u=f"https://sourcify.dev/server/v2/contract/8453/{addr}"
        r=requests.get(u,params={"fields":"compilation,runtimeBytecode.onchainBytecode,sources"},timeout=12);r.raise_for_status();j=r.json()
        if j.get("runtimeMatch") not in ("match","exact_match") or j["runtimeBytecode"]["onchainBytecode"].lower()!=code.lower():raise ValueError("runtime differs from source")
        files={n:"0x"+keccak(text=s["content"]).hex() for n,s in j["sources"].items() if any(x in n for x in ("FiatToken","Proxy.sol","WETH9.sol","Blacklistable","Pausable"))}
        return dict(address=addr,runtime="0x"+keccak(bytes.fromhex(code[2:])).hex(),source=u,name=j["compilation"]["fullyQualifiedName"],files=files)
    try:
        head=rpc("eth_getBlockByNumber",[hex(directory["block"]),False])
        if rpc("eth_chainId",[])!="0x2105" or head["hash"]!=tag["blockHash"]:raise ValueError("wrong canonical history")
        result={}
        for name,addr in TOKENS.items():
            row=contract(addr);raw={k:rpc("eth_getStorageAt",[addr,v,tag]) for k,v in SLOTS.items()};row["storage"]=raw
            if name=="WETH":
                if any(int(x,16) for x in raw.values()):raise ValueError("unexpected WETH proxy")
            else:
                for word in raw.values():
                    if not 0<int(word,16)<2**160:raise ValueError("unexpected zero or packed proxy role")
                row["implementation"]=contract("0x"+raw["implementation"][-40:]);row["admin"]="0x"+raw["admin"][-40:]
            result[name]=row
        history=json.loads(gzip.decompress((DATA/"ryze_base_verified_intents.json.gz").read_bytes()))["cases"]
        fees=[]
        for row in history:
            at=dict(blockHash=row["parent_hash"],requireCanonical=True)
            call={"to":row["router"],"data":"0x"+keccak(text="intentFee()")[:4].hex()}
            raw=rpc("eth_call",[call,at])
            fees.append(dict(transaction=row["transaction"],selector=at,call=call,raw=raw,usd_wad=str(int(raw,16))))
        return dict(historical_intent_fees=fees,chain=8453,height=directory["block"],selector=tag,state_root=head["stateRoot"],slots=SLOTS,tokens=result)
    finally:ws.close()

if __name__=="__main__":
    (DATA/"ryze_base_tokens.json").write_text(json.dumps(collect(),indent=2)+"\n",encoding="utf-8")
