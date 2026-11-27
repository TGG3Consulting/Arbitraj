"""Bind historical Ryze spot intents to their EIP-712 signer, nonce, ERC20 payments and canonical receipt."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_base_slipstream_executor import ROOT

TXS = (
    "0x377511e15584e680120c4375b5d10343ac98d805ee5616906438e9d4bdb25881",
    "0xb4ac83bd0496d7be8975554631484a770b7afce23d48ffe5d0a058e412374804",
    "0xdc5356393c01195e588adf09825477e0488943338b201d5111fad5cd446b9cd6",
)
ABI_URL = "https://documentation.ryze.pro/abi/amm/MultiHopRouter.json"
HOP = "Hop(address pool,address tokenIn,address tokenOut)"
INTENT = "SwapIntent(address user,address tokenIn,address tokenOut,uint256 amountIn,uint256 minAmountOut,Hop[] path,address recipient,uint256 deadline,uint256 nonce)"
DOMAIN = "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
SWAP = "IntentSwapExecuted(address,address,address,uint256,uint256,address,uint256,uint256)"
TRANSFER = "Transfer(address,address,uint256)"


def topic(name):
    return "0x" + keccak(text=name).hex()


def abi_type(d):
    return ("("+",".join(map(abi_type,d["components"]))+")"+d["type"][5:]) if "components" in d else d["type"]


def capture(url):
    session=requests.Session()
    def rpc(method,params):
        if method not in {"eth_getTransactionByHash","eth_getTransactionReceipt","eth_getBlockByHash","eth_getBlockByNumber","eth_call","eth_chainId"}:
            raise ValueError(method)
        response=session.post(url,json={"method":method,"params":params,"id":1,"jsonrpc":"2.0"},timeout=20)
        response.raise_for_status()
        data=response.json()
        if data.get("result") is None:raise RuntimeError(str(data.get("error"))[:180])
        return data["result"]
    if rpc("eth_chainId",[])!="0x2105":raise ValueError("wrong network")
    directory=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base_recovered_markets.json").read_bytes())["ryze"]
    router=directory["router"]["address"].lower()
    source=session.get(ABI_URL,timeout=15);source.raise_for_status()
    abi=next(f for f in source.json() if f.get("name")=="executeSwapIntents")
    types=list(map(abi_type,abi["inputs"]))
    selector=topic("executeSwapIntents("+",".join(types)+")")[:10]
    cases=[]
    for tx_hash in TXS:
        tx=rpc("eth_getTransactionByHash",[tx_hash])
        receipt=rpc("eth_getTransactionReceipt",[tx_hash])
        head=rpc("eth_getBlockByNumber",[tx["blockNumber"],False])
        if (tx["blockHash"]!=head["hash"] or receipt["blockHash"]!=head["hash"] or receipt["status"]!="0x1"
                or tx["to"].lower()!=router or tx["input"][:10]!=selector or tx_hash!=receipt["transactionHash"]):
            raise ValueError("inclusion or public router changed")
        parent=rpc("eth_getBlockByHash",[head["parentHash"],False])
        tag=lambda h:{"blockHash":h["hash"],"requireCanonical":True}
        def query(to,data,anchor=head):
            return rpc("eth_call",[{"to":to,"data":data},tag(anchor)])
        payload=bytes.fromhex(tx["input"][10:])
        signed,_,_=decode(types,payload)
        if len(signed)!=1:raise ValueError("single-intent historical control")
        intent,signature=signed[0]
        user,source,target,budget,minimum,path,recipient,deadline,nonce=intent
        if len(signature)!=65 or signature[64] not in (27,28) or int(head["timestamp"],16)>deadline:
            raise ValueError("historical signer or deadline")
        domain=decode(["bytes1","string","string","uint256","address","bytes32","uint256[]"],
                      bytes.fromhex(query(router,topic("eip712Domain()")[:10],parent)[2:]))
        fields,name,version,chain,contract,salt,extensions=domain
        if (fields,name,version,chain,contract,salt,extensions)!=(b"\x0f","MultiHopRouter","1",8453,router,bytes(32),()):
            raise ValueError(("typed domain",name,version,chain,contract))
        domain_hash=keccak(encode(["bytes32","bytes32","bytes32","uint256","address"],
                           [keccak(text=DOMAIN),keccak(text=name),keccak(text=version),chain,router]))
        path_hash=keccak(b"".join(keccak(encode(["bytes32","address","address","address"],[keccak(text=HOP),*hop])) for hop in path))
        struct_hash=keccak(encode(["bytes32","address","address","address","uint256","uint256","bytes32","address","uint256","uint256"],
                    [keccak(text=INTENT+HOP),user,source,target,budget,minimum,path_hash,recipient,deadline,nonce]))
        digest=keccak(b"\x19\x01"+domain_hash+struct_hash)
        encoded=digest+int(signature[64]).to_bytes(32,"big")+signature[:64]
        recovered=query("0x"+"00"*19+"01","0x"+encoded.hex(),parent)
        nonce_call=topic("swapNonces(address)")[:10]+encode(["address"],[user]).hex()
        nonce_pair=[int(query(router,nonce_call,h),16) for h in (parent,head)]
        if recovered[-40:]!=user[2:] or nonce_pair!=[nonce,nonce+1]:raise ValueError(("authority",nonce_pair,user,recovered))
        logs=receipt["logs"]
        matches=[v for v in logs if v["address"]==router and v["topics"][0]==topic(SWAP)]
        if len(matches)!=1 or ["0x"+s[-40:] for s in matches[0]["topics"][1:]]!=[user,source,target]:
            raise ValueError("unexpected intent event")
        spent,net,dest,used,fee=decode(["uint256","uint256","address","uint256","uint256"],bytes.fromhex(matches[0]["data"][2:]))
        if (spent,dest,used)!=(budget,recipient,nonce) or net<minimum:raise ValueError("payout outside intent")
        transfers=[(l["address"],"0x"+l["topics"][1][-40:],"0x"+l["topics"][2][-40:],int(l["data"],16))
                   for l in logs if len(l["topics"])==3 and l["topics"][0]==topic(TRANSFER)]
        def delta(token,owner):
            return sum(n*(int(out==owner)-int(inp==owner)) for a,inp,out,n in transfers if a==token)
        if (delta(source,user),delta(target,recipient))!=(-spent,net):
            raise ValueError("ERC20 wallet deltas differ from router")
        cases.append(dict(transaction=tx_hash,block=int(head["number"],16),block_hash=head["hash"],parent_hash=parent["hash"],
                          root=head["stateRoot"],router=router,digest="0x"+digest.hex(),user=user,recipient=recipient,
                          token_in=source,token_out=target,input=str(budget),minimum=str(minimum),net=str(net),
                          fee_in_input=str(fee),deadline=deadline,nonce=nonce,nonces=nonce_pair,path=path,
                          event=matches[0],transfers=transfers,gas_paid_by=tx["from"]))
    return {"chain":8453,"abi":ABI_URL,"selector":selector,"cases":cases}


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("out",type=Path)
    args=cli.parse_args()
    for endpoint in ("https://base-rpc.publicnode.com","https://mainnet.base.org"):
        try:
            data=capture(endpoint)
            break
        except (requests.RequestException,RuntimeError) as e:print(str(e)[:160],flush=True)
    else:raise RuntimeError("archive unavailable")
    args.out.write_text(json.dumps(data,separators=(",",":")),encoding="utf8")
