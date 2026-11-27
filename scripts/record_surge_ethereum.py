"""Два последовательных канонически привязанных кадра для собственного кошелька SURGE, лимиты и реальная выплата ETH."""
import argparse
import json
from pathlib import Path

import requests
from eth_utils import keccak
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, OWNER, rpc
from record_ethervista_wallets import tx,integer
from record_ethereum_x7 import ENDPOINTS,ROOT_HASH,verified

TOKEN="0xcd682ef09d07668d49a8103ddd65ff54aebfbfde"
MC="0xca11bde05977b3631167028862be2a173976ca11"
SOLD="0x"+keccak(text="Sold(address,address,uint256,uint256,uint256)").hex()
BOUGHT="0x"+keccak(text="Bought(address,address,uint256,uint256,uint256)").hex()

def capture(url):
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes());tag=snap["selector"]
    c=Capture(dict(snap,endpoint=url));h=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
    if rpc(url,"eth_chainId",[])!="0x1" or h["hash"]!=tag["blockHash"] or h["stateRoot"]!=ROOT_HASH:raise ValueError("different Ethereum head")
    origin,source,code=verified(c,TOKEN,"SURGE")
    if rpc(url,"eth_getCode",[OWNER,tag])!="0x":raise ValueError("isolated payer is not empty code")
    fields=["liquidity","liqConst","maxBag","taxBalance","buyMul","sellMul","DIVISOR","TRADE_OPEN_TIME","totalTx","getBNBPrice"]
    state={key:str(c.one(TOKEN,key+"()",returns=["uint256"])[0]) for key in fields}
    for key,method,arg,typ in [("stock","balanceOf(address)",TOKEN,"uint256"),("wallet","balanceOf(address)",OWNER,"uint256"),
                               ("fee_exempt","isFeeExempt(address)",OWNER,"bool"),("limit_exempt","isTxLimitExempt(address)",OWNER,"bool")]:
        value,=c.one(TOKEN,method,["address"],[arg],[typ]);state[key]=value if isinstance(value,bool) else str(value)
    if int(state["wallet"])!=0:raise ValueError("payer already held SURGE")
    liq,stock,k,den=[int(state[key]) for key in ("liquidity","stock","liqConst","DIVISOR")]
    def balance(asset,who):
        return tx(MC,"getEthBalance(address)",["address"],[who]) if asset=="eth" else tx(TOKEN,"balanceOf(address)",["address"],[who])
    probes=[balance(asset,who) for who in (OWNER,TOKEN) for asset in ("eth","token")]
    def sim(first,second):
        groups=[{"stateOverrides":{OWNER:{"balance":hex(10**18)}},"calls":first},{"calls":second}]
        blocks=rpc(url,"eth_simulateV1",[{"blockStateCalls":groups},tag])
        if len(blocks)!=2 or blocks[0]["parentHash"]!=tag["blockHash"] or blocks[1]["parentHash"]!=blocks[0]["hash"]:
            raise ValueError("simulation parent differs")
        for i,block in enumerate(blocks,1):
            if int(block["number"],16)!=int(h["number"],16)+i or len(block["calls"])!=len(groups[i-1]["calls"]):
                raise ValueError("unexpected calls or block number")
        return blocks
    def values(xs):return [integer(r) for r in xs]
    cases=[];end=int(h["timestamp"],16)+300
    for amount in (10**15,10**16):
        effective=amount if state["fee_exempt"] else amount*int(state["buyMul"])//den
        bought=stock-k//(liq+effective)
        gross=liq+effective-k//stock
        tax=0 if state["fee_exempt"] else gross*(den-int(state["sellMul"]))//den
        net=gross-tax
        buy=lambda minimum,deadline:tx(TOKEN,"_buy(uint256,uint256)",["uint256","uint256"],[minimum,deadline],amount)
        sell=lambda minimum,deadline:tx(TOKEN,"_sell(uint256,uint256,uint256)",["uint256"]*3,[bought,deadline,minimum])
        first=[*probes,buy(bought+1,end),buy(bought,1),*probes,buy(bought,end),*probes,sell(1,end),*probes]
        second=[*probes,sell(gross+1,end),sell(gross,1),*probes,sell(gross,end),*probes]
        frames=sim(first,second);a,b=[f["calls"] for f in frames]
        groups=[[a,[4,5,15]],[b,[4,5]]]
        for rows,failures in groups:
            if [i for i,row in enumerate(rows) if row["status"]!="0x1"]!=failures:raise ValueError(("unexpected transition",[(i,row.get("error")) for i,row in enumerate(rows) if row["status"]!="0x1"]))
            if values(rows[:4])!=values(rows[6:10]):raise ValueError("expired or high floor changed balances")
        before=values(a[:4]);after=values(a[11:15]);end_state=values(b[11:15])
        if after!=values(a[16:20]) or after!=values(b[:4]):raise ValueError("in-block reversal or continuity")
        if [before[0]-after[0],after[1]-before[1],after[2]-before[2],before[3]-after[3]]!=[amount,bought,amount,bought]:raise ValueError("buy payment differs")
        if [end_state[0]-after[0],after[1]-end_state[1],after[2]-end_state[2],end_state[3]-after[3]]!=[net,bought,net,bought]:raise ValueError("sell payment differs")
        for sig,settled in [(BOUGHT,a[10]),(SOLD,b[10])]:
            if len([log for log in settled["logs"] if log["address"].lower()==TOKEN and log["topics"][0]==sig])!=1:
                raise ValueError("missing settlement event")
        cases.append(dict(input=str(amount),bought=str(bought),gross=str(gross),received=str(net),
                   before=list(map(str,before)),after_buy=list(map(str,after)),after_sell=list(map(str,end_state)),
                   buy=dict(calldata=first[10],logs=a[10]["logs"],gas=int(a[10]["gasUsed"],16),minimum_plus_one=a[4]["error"],expired=a[5]["error"]),
                   sell=dict(calldata=second[10],logs=b[10]["logs"],gas=int(b[10]["gasUsed"],16),minimum_plus_one=b[4]["error"],expired=b[5]["error"],same_block=a[15]["error"])))
    return dict(chain=1,selector=tag,header_root=ROOT_HASH,height=snap["block"],address=TOKEN,runtime="0x"+keccak(code).hex(),source=origin,
                creation=source["compilation"]["name"],sender=OWNER,state=state,cases=cases)

if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__);cli.add_argument("output",type=Path);a=cli.parse_args();failures=[]
    for url in ENDPOINTS:
        try:
            data=capture(url);a.output.write_text(json.dumps(data,separators=(",",":")),encoding="utf8");break
        except (ValueError,KeyError,RuntimeError,requests.RequestException) as e:failures.append((url,str(e)[:350]))
    else:raise RuntimeError(failures)
