"""Count the investor's own USDT through two original V3 pools and an integral Aktionariat share."""
import argparse
import gzip
import json
from pathlib import Path

from eth_abi import encode
from eth_utils import keccak
from record_aktionariat_protected import MODULE, DIRECTORY, BASELINE
from record_aktionariat_wallet import FUND, SWAP, UINT, WETH, approve
from record_base_slipstream_executor import EXEC, OWNER, RUNTIME, STEP, rpc, simulate_block
from record_bitswap_ethereum import balance, token_flow, tx

USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
FACTORY = "0x1f98431c8ad98523631ae4a59f267346ea31f984"
QUOTER = "0xb27308f9f90d607463bb33ea1bebb41c27ce5ab6"

def capture(url):
    native=json.loads(gzip.decompress(BASELINE.read_bytes()))
    registry=json.loads(gzip.decompress(DIRECTORY.read_bytes()))
    paid=json.loads(gzip.decompress(DIRECTORY.with_name("aktionariat_ethereum_protected.json.gz").read_bytes()))
    m=paid["manifest"];tag=paid["selector"]
    assert registry["selector"]==native["selector"]==tag and rpc(url,"eth_chainId",[])=="0x1"
    block=rpc(url,"eth_getBlockByHash",[tag["blockHash"],False])
    assert block["stateRoot"]==registry["state_root"] and m["sourceBlock"]==block["hash"]
    code=rpc(url,"eth_call",[{"from":OWNER,"data":m["data"],"gas":hex(6000000)},tag])
    assert m["runtimeKeccak"]=="0x"+keccak(hexstr=code).hex()
    assert all(rpc(url,"eth_getCode",[a,tag])=="0x" for a in (EXEC,OWNER,MODULE))
    word=lambda x: "0x"+format(x,"064x")
    env={OWNER:{"balance":hex(100*10**18)},MODULE:{"code":code},
         EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),
               "stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(WETH,16))}}}
    end=int(block["timestamp"],16)+900
    def run(calls):return simulate_block(url,tag,env,calls)
    def pool(x,y,fee):
        res,=run([tx(FACTORY,"getPool(address,address,uint24)",["address","address","uint24"],[x,y,fee])])
        assert res["status"]=="0x1" and int(res["returnData"],16)
        return "0x"+res["returnData"][-40:]
    def execute(steps,qty,floor):
        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address",UINT,"address",UINT,UINT,STEP+"[]"],[steps[0][2],qty,steps[-1][3],floor,end,steps])
    weth_usdt=pool(WETH,USDT,500)
    deposit=8*10**18
    fund=[tx(WETH,"deposit()",value=deposit),approve(WETH,FUND,deposit),tx(FUND,"exactInputSingle("+SWAP+")",[SWAP],[(WETH,USDT,3000,OWNER,end,deposit,1,0)])]
    controls=[]
    for broker in ["0xcf1f5270916e41f5cf4221f68798438c3e5404e6","0x9f61e2b3fe63aa013b606160d0525fc141d495bf"]:
        market=registry["markets"][broker];cash=market["cash"];share=market["share"]
        weth_cash=pool(WETH,cash,3000)
        legs=[(1,weth_usdt,USDT,WETH,10000,b""),(1,weth_cash,WETH,cash,10000,b""),
              (17,MODULE,cash,share,10000,encode(["address"],[broker]))]
        reverse=[(k,p,b,a,s,d) for k,p,a,b,s,d in reversed(legs)]
        path=bytes.fromhex(cash[2:])+int(3000).to_bytes(3,"big")+bytes.fromhex(WETH[2:])+int(500).to_bytes(3,"big")+bytes.fromhex(USDT[2:])
        for units in (1,10):
            baseline=next(r for r in native["controls"] if r["market"]==broker and r["units"]==units and r["mode"]=="hub")
            cost=int(baseline["buy_spent"])
            quote=tx(QUOTER,"quoteExactOutput(bytes,uint256)",["bytes",UINT],[path,cost])
            funded=run([*fund,quote,balance(USDT,OWNER)])
            assert all(r["status"]=="0x1" for r in funded)
            budget=int(funded[-2]["returnData"],16)
            assert 0<budget<=int(funded[-1]["returnData"],16)
            buy=execute(legs,budget,units); sell=execute(reverse,units,1)
            accounts=[balance(t,OWNER) for t in [USDT,WETH,cash,share]]
            before=[*fund,approve(USDT,EXEC,budget),approve(share,EXEC,units),*accounts]
            r=run([*before,buy,*accounts,sell,*accounts])
            assert all(x["status"]=="0x1" for x in r)
            start=len(before);buy_receipt=r[start];sell_receipt=r[start+5]
            have=lambda p:[int(x["returnData"],16) for x in r[p:p+4]]
            old,mid,new=have(start-4),have(start+1),have(start+6)
            got=int(sell_receipt["returnData"],16)
            assert got>0 and old[0]-mid[0]==budget and new[0]-mid[0]==got and mid[3]-old[3]==units==mid[3]-new[3]
            assert old[1]==mid[1]==new[1] and new[2]==mid[2] and new[2]>=old[2]
            for receipt,asset,expected in [(buy_receipt,share,units),(buy_receipt,USDT,-budget),(sell_receipt,share,-units),(sell_receipt,USDT,got)]:
                assert token_flow(receipt["logs"],asset,OWNER)==expected
            probe=run([*before,execute(legs,budget,units+1),buy,execute(reverse,units,got+1),execute(reverse,units,got),*accounts])
            assert [x["status"] for x in probe[start:start+4]]==["0x0","0x1","0x0","0x1"]
            assert [int(v["returnData"],16) for v in probe[-4:]]==new
            controls.append(dict(broker=broker,cash=cash,share=share,units=units,pools=[weth_usdt,weth_cash],quote_path="0x"+path.hex(),
                budget=str(budget),returned=str(got),cash_dust=str(new[2]-old[2]),before=list(map(str,old)),after_buy=list(map(str,mid)),
                after=list(map(str,new)),gas=[int(x["gasUsed"],16) for x in (buy_receipt,sell_receipt)],logs=[buy_receipt["logs"],sell_receipt["logs"]],
                buy=buy,sell=execute(reverse,units,got),refusals=[probe[start]["error"],probe[start+2]["error"]]))
    return dict(selector=tag,owner=OWNER,module=MODULE,executor=EXEC,factory=FACTORY,controls=controls)
if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path);a=p.parse_args()
    failures=[]
    for endpoint in ("https://rpc.mevblocker.io","https://eth.drpc.org"):
        try:data=capture(endpoint);break
        except (RuntimeError, ValueError, OSError) as error:failures.append(str(error)[:160])
    else:raise RuntimeError(failures)
    a.output.write_text(json.dumps(data,separators=(",",":")),encoding="utf8")
