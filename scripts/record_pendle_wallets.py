"""Ethereum Pendle. Покупка PT собственными USDC и погашение позиции через обмен обратно в те же USDC до expiry."""
import argparse
import gzip
import json
from pathlib import Path
import requests
from eth_abi import encode,decode
from eth_utils import keccak
from record_base_slipstream_executor import OWNER,ROOT,simulate_block
from record_bitswap_ethereum import BLOCK,ENDPOINTS,FUNDING,WETH,tx,balance,token_flow,read_batch
from record_ethereum_x7 import ROOT_HASH
from record_pendle_registry import AT,ROUTER,ZERO
USDC="0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
TOKEN="(address,uint256,address,address,(uint8,address,bytes,bool))"
ORDER="(uint256,uint256,uint256,uint8,address,address,address,address,uint256,uint256,uint256,bytes)"
FILL="("+ORDER+",bytes,uint256)"
LIMIT="(address,uint256,"+FILL+"[],"+FILL+"[],bytes)"
BUY="swapExactTokenForPtSimple(address,address,uint256,"+TOKEN+")"
SELL="swapExactPtForToken(address,address,uint256,"+TOKEN+","+LIMIT+")"
CORE_SLOT=0xf168c5b0cb4aca9a68f931815c18a144c61ad01d6dd7ca15bd6741672a0ab800
def payment(market,quantity,minimum,buy):
    token=(USDC,quantity if buy else minimum,USDC,ZERO,(0,ZERO,b"",False))
    if buy:return tx(ROUTER,BUY,["address","address","uint256",TOKEN],[OWNER,market,minimum,token])
    return tx(ROUTER,SELL,["address","address","uint256",TOKEN,LIMIT],[OWNER,market,quantity,token,(ZERO,0,[],[],b"")])
def capture(url):
    f=ROOT/"tests/data/first_run_dex_2026-11-26/pendle_ethereum_registry.json.gz"
    directory=json.loads(gzip.decompress(f.read_bytes())); sy=directory["standardized_yield"]
    chain,head,user=read_batch(url,[("eth_chainId",[]),("eth_getBlockByHash",[BLOCK,False]),("eth_getCode",[OWNER,AT])])
    if chain!="0x1" or head["stateRoot"]!=ROOT_HASH or user!="0x" or directory["selector"]!=AT:raise ValueError("block or simulation owner")
    env={OWNER:{"balance":hex(10**19)}};run=lambda calls:simulate_block(url,AT,env,calls)
    def binding(sig):
        selector=keccak(text=sig)[:4];slot=keccak(encode(["bytes4","uint256"],[selector,CORE_SLOT+2]))
        prop=read_batch(url,[("eth_getStorageAt",[ROUTER,"0x"+slot.hex(),AT])])[0];a="0x"+prop[-40:]
        code=read_batch(url,[("eth_getCode",[a,AT])])[0];return dict(facet=a,bytes=(len(code)-2)//2,runtime_hash="0x"+keccak(hexstr=code).hex())
    bindings={s:binding(s) for s in [BUY,SELL]};result=[]
    for r in directory["registrations"]:
        if not r.get("series") or not r.get("router_state") or min(map(int,r["physical"]))<=0 or min(map(int,r["router_state"][:3]))<=0:continue
        syaddr,pt,_=r["tokens"];allowed=sy[syaddr]
        if USDC not in (allowed["in"] or []) or USDC not in (allowed["out"] or []):continue
        m=r["market"]
        for size in [10**6,10*10**6]:
            fund=tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[size,[WETH,USDC],OWNER,directory["timestamp"]+300],10**18)
            approve=lambda token,n:tx(token,"approve(address,uint256)",["address","uint256"],[ROUTER,n])
            case={"market":m,"pt":pt,"sy":syaddr,"size":str(size),"expiry":r["expiry"]};result.append(case)
            prefix=[fund]
            for buy in (True,False):
                token_in,token_out=(USDC,pt) if buy else (pt,USDC)
                amount=size if buy else int(case["buy"]["net"])
                base=prefix+[approve(token_in,amount)]
                reads=[balance(token_in,OWNER),balance(token_out,OWNER),balance(syaddr,OWNER),balance(r["tokens"][2],OWNER)]
                def probe(minimum):
                    op=payment(m,amount,minimum,buy);rs=run(base+reads+[op]+reads);idx=len(base)+len(reads)
                    if any(x["status"]!="0x1" for j,x in enumerate(rs) if j!=idx):raise ValueError("funds, approvals or reads failed")
                    vals=lambda a:[int(x["returnData"],16) for x in a]
                    return op,rs[idx],vals(rs[len(base):idx]),vals(rs[idx+1:]),int(rs[len(base)-1]["gasUsed"],16)
                _,q,_,_,_=probe(1);leg="buy" if buy else "sell"
                if q["status"]!="0x1":case[leg]={"stage":"unavailable","error":q.get("error")};break
                received=token_flow(q["logs"],token_out,OWNER)
                if received<=0:case[leg]={"stage":"no receiver credit"};break
                op,res,before,after,approval=probe(received)
                if res["status"]!="0x1" or before[0]-after[0]!=amount or after[1]-before[1]!=received or before[2:]!=after[2:] or token_flow(res["logs"],token_in,OWNER)!=-amount:raise ValueError("wallet debit, credit or extra derivative")
                _,strict,bs,af,_=probe(received+1)
                if strict["status"]!="0x0" or bs!=af:raise ValueError("terminal minimum not enforced")
                case[leg]={"stage":"accepted","input":token_in,"output":token_out,"paid":str(amount),"net":str(received),"before":list(map(str,before)),"after":list(map(str,after)),
                           "gas":int(res["gasUsed"],16),"approval_gas":approval,"returned":res["returnData"],"logs":res["logs"],"transaction":op,"strict":strict.get("error")}
                prefix=base+[op]
    return {"selector":AT,"state_root":ROOT_HASH,"payer":OWNER,"router":ROUTER,"bindings":bindings,"terminal":USDC,"cases":result}
if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path);opts=p.parse_args();failures=[]
    for url in ENDPOINTS:
        try:
            output=capture(url);opts.output.write_text(json.dumps(output,separators=(",",":")),encoding="utf8");break
        except (ValueError,KeyError,RuntimeError,requests.RequestException) as e:failures.append(str(e)[:240])
    else:raise RuntimeError(failures)
