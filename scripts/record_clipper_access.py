"""Check public Clipper/Blade RFQ access and recover the designated signer of included settlements."""
import argparse
import datetime
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_account import Account
from eth_utils import keccak
from record_base_slipstream_executor import OWNER

NETS = {
    1: ("0x655edce464cc797526600a462a8154650eee4b77","0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2",
        "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",["https://eth.drpc.org","https://rpc.mevblocker.io"]),
    8453: ("0xb32d856cad3d2ef07c94867a800035e37241247c","0x4200000000000000000000000000000000000006",
           "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",["https://base.gateway.tenderly.co","https://base.drpc.org"])
}
TRANSACTIONS = {
    1: ["0x4c9e1d676ae47dac7aa01d4a4b39204352565393f541614f38a445b44e1c40e4",
        "0xeb317c5698fca7c068ea509457826df5c3fd18adb3d7e267597bcf76701d4d6a",
        "0xe1e03d8c5d011f77233891676c303c09f016da78c3d0429e15d07a9e0e4145cb",
        "0x1d355632b9c72856ee163fd4c92849c7bf890b3694fa67d7edace93015494d66"],
    8453: ["0x56ca81f9cd97fd3243134f4667599fb19bd8118743466abdd59dda72875d0e31",
           "0xace5dd0c01aed15f05bae646f3e139db92e41a3ae6a747c0704e35c9bdcb93df",
           "0x26e49ee24cfb92a75772418d0ab89c6a7a32059a154b6d448fea420a6fda6c1c",
           "0x49b1a115f005c45a663e73940a05530e86faa2e2ec71ad3058b34f05b9bc9c1e"],
}
OFFER = keccak(text="OfferStruct(address input_token,address output_token,uint256 input_amount,uint256 output_amount,uint256 good_until,address destination_address)")
DOMAIN = keccak(text="EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")
SWAP = "0x"+keccak(text="Swapped(address,address,address,uint256,uint256,bytes)").hex()
TRANSFER = "0x"+keccak(text="Transfer(address,address,uint256)").hex()
def raw(x): return bytes.fromhex(x.removeprefix("0x"))

def rpc(chain,method,params):
    if method not in {"eth_chainId","eth_getTransactionByHash","eth_getTransactionReceipt","eth_getBlockByHash",
                      "eth_call","trace_transaction"}:
        raise ValueError("read method required")
    failures=[]
    for url in NETS[chain][3]:
        try:
            r=requests.post(url,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=12)
            r.raise_for_status()
            row=r.json()
            if row.get("result") is not None:
                return row["result"]
            failures.append(str(row.get("error"))[:130])
        except (requests.RequestException,ValueError) as e:
            failures.append(str(e)[:120])
    raise ValueError((method,failures))

def history(chain,txid):
    pool,wrapped,_,_=NETS[chain]
    tx=rpc(chain,"eth_getTransactionByHash",[txid])
    receipt=rpc(chain,"eth_getTransactionReceipt",[txid])
    head=rpc(chain,"eth_getBlockByHash",[tx["blockHash"],False])
    assert head["hash"]==receipt["blockHash"]==tx["blockHash"] and receipt["status"]=="0x1"
    assert head["transactions"][int(tx["transactionIndex"],16)]==txid and int(tx["chainId"],16)==chain
    tag={"blockHash":head["hash"],"requireCanonical":True}
    def read(sig):
        return raw(rpc(chain,"eth_call",[{"to":pool,"data":"0x"+keccak(text=sig)[:4].hex()},tag]))
    signer=decode(["address"],read("DESIGNATED_SIGNER()"))[0]
    assert decode(["address"],read("WRAPPER_CONTRACT()"))[0]==wrapped
    trace=rpc(chain,"trace_transaction",[txid])
    calls=[r["action"] for r in trace if not r.get("error") and r["action"].get("to")==pool and
           r["action"].get("input","")[:10] in ("0x2b651a6c","0x27a9b424","0x4cb6864c")]
    call,=calls
    wire=raw(call["input"]);sel=wire[:4].hex()
    # Some aggregator encoders omit right-padding after the final auxiliary bytes.
    fields=["address"]*(2 if sel=="2b651a6c" else 1)+["uint256","uint256","uint256","address","(uint8,bytes32,bytes32)","bytes"]
    args=decode(fields,wire[4:],strict=False)
    if sel=="27a9b424": args=(wrapped,)+args
    if sel=="4cb6864c": args=(args[0],wrapped)+args[1:]
    a,b,amount,payout,packed,to,sig,_=args
    domain=keccak(encode(["bytes32","bytes32","bytes32","uint256","address"],[DOMAIN,keccak(text="ClipperDirect"),keccak(text="1.0.0"),chain,pool]))
    typed=keccak(encode(["bytes32","address","address","uint256","uint256","uint256","address"],[OFFER,*args[:6]]))
    digest=keccak(bytes.fromhex("1901")+domain+typed)
    modes=[m for m,d in [("destination",digest),("origin+destination",keccak(digest+raw(tx["from"])))]
           if Account._recover_hash(d,vrs=sig).lower()==signer]
    mode,=modes
    event,=[l for l in receipt["logs"] if l["address"]==pool and l["topics"][:1]==[SWAP]]
    spent,net,_=decode(["uint256","uint256","bytes"],raw(event["data"]),strict=False)
    event_in,event_out,beneficiary=["0x"+topic[-40:] for topic in event["topics"][1:]]
    assert beneficiary==to and spent>0 and net>0
    eth="0x"+"00"*20
    assert (event_in if event_in!=eth else wrapped,event_out if event_out!=eth else wrapped)==(a,b)
    transfers=[(l["address"],"0x"+l["topics"][1][-40:],"0x"+l["topics"][2][-40:],int(l["data"],16)) for l in receipt["logs"]
               if len(l["topics"])==3 and l["topics"][0]==TRANSFER]
    if sel=="27a9b424":
        assert int(call["value"],16)==spent
        funding=call["from"]
    else:
        funding,=[origin for asset,origin,target,value in transfers if (asset,target,value)==(a,pool,spent)]
    if sel=="4cb6864c":
        delivery=[r for r in trace if not r.get("error") and r["action"].get("from")==pool and
                  r["action"].get("to")==to and int(r["action"].get("value","0x0"),16)==net]
        assert len(delivery)==1
    else:
        assert transfers.count((b,pool,to,net))==1
    return dict(chain=chain,block=int(head["number"],16),hash=head["hash"],transaction=txid,outer=tx["to"],
                caller=call["from"],origin=tx["from"],funding=funding,pool=pool,signer=signer,mode=mode,selector="0x"+sel,
                digest="0x"+digest.hex(),packed_good_until=str(packed),expires=packed & 0xffffffff,
                timestamp=int(head["timestamp"],16),input=a,output=b,recipient=to,quoted=[str(amount),str(payout)],
                delivered=[str(spent),str(net)],native_input=sel=="27a9b424",native_output=sel=="4cb6864c")

def public(chain):
    pool,wrapped,usdc,_=NETS[chain]
    pools=pool if chain==1 else pool+",0x663fa8731045765067e2b5b6acb19be2ce974c7f"
    def ask(method,url,data=None):
        try:
            r=requests.request(method,url,params=data if method=="GET" else None,json=data if method=="POST" else None,timeout=8)
            j=r.json()
            if isinstance(j,list):
                return dict(status=r.status_code,count=len(j),named=[v for v in j if any(k in str(v).lower() for k in ("blade","clipper"))])
            if "dexList" in url:
                items=j["data"]
                return dict(status=r.status_code,count=len(items),named=[v for v in items if any(k in str(v).lower() for k in ("blade","clipper"))])
            # Publish only status and digests. An accepted quote or signed payload belongs to its recipient.
            return dict(status=r.status_code,result=j.get("status",j.get("code")),fields=sorted(j)[:18],
                        quote_or_transaction=bool(j.get("signature") or j.get("tx")),
                        error=str(j.get("detail",j.get("error",j.get("message",""))))[:160])
        except (requests.RequestException,ValueError,KeyError) as exc:
            return {"unavailable":type(exc).__name__}
    out=dict(sushi=ask("GET",f"https://api.sushi.com/liquidity-providers/v7/{chain}"),
             openocean=ask("GET",f"https://open-api.openocean.finance/v4/{chain}/dexList"),
             velora=ask("GET","https://api.paraswap.io/adapters/list",{"network":chain}),attempts=[])
    for a,b,n,t,u in [("ETH","USDC",str(10**15),wrapped,usdc),("USDC","ETH","1000000",usdc,wrapped)]:
        terms=dict(input_asset_symbol=a,output_asset_symbol=b,input_amount=n,time_in_seconds=30)
        sushi=dict(tokenIn=t,tokenOut=u,amount=n,sender=OWNER,recipient=OWNER,onlyPools=pools)
        out["attempts"].append(dict(input=a,output=b,amount=n,public_sushi=ask("GET",f"https://api.sushi.com/swap/v7/{chain}",sushi),
            v1=ask("POST","https://api.clipper.exchange/rfq/quote",dict(terms,chain_id=chain)),
            v2=ask("GET",f"https://blade-api.sushi.com/rfq/v2/quote/{chain}",terms),
            combined=ask("GET",f"https://blade-api.sushi.com/rfq/v2/quote-sign/{chain}",
                dict(terms,destination_address=OWNER,sender_address=OWNER,calldata="true"))))
    return out

def dns_status():
    results={}
    for host in ("api.clipper.exchange","blade-api.sushi.com"):
        records={}
        for kind in ("A","AAAA"):
            r=requests.get("https://dns.google/resolve",params=dict(name=host,type=kind),timeout=8)
            r.raise_for_status()
            answer=r.json()
            records[kind]={"status":answer["Status"],"values":[(v["type"],v["data"]) for v in answer.get("Answer",[])]}
        results[host]=records
    return results

if __name__=="__main__":
    ap=argparse.ArgumentParser();ap.add_argument("output",type=Path);args=ap.parse_args()
    result={"observed":datetime.datetime.now(datetime.timezone.utc).isoformat(),"payer":OWNER,"dns":dns_status(),"networks":{}}
    for chain,transactions in TRANSACTIONS.items():
        assert int(rpc(chain,"eth_chainId",[]),16)==chain
        result["networks"][str(chain)]=dict(api=public(chain),original=[history(chain,t) for t in transactions])
    args.output.write_text(json.dumps(result,separators=(",",":")),encoding="utf8")
