"""Price the original Ethereum V1 exchanges in ETH, settle in native ETH or WETH and compare both wallets."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, rpc, simulate_block
from record_bitswap_ethereum import balance, tx
from record_ethereum_v2_factories import WETH, STABLES, FUNDING
from record_safeswap_protected import runtime
from record_v2_chain_pools import minimum

FACTORY="0xc0a47dfe034b400b47bdad5fecda2621de6c4d95"
DAI="0x6b175474e89094c44da98b954eedeac495271d0f"
MULTI="0xca11bde05977b3631167028862be2a173976ca11"
MODULE="0x"+keccak(text="isolated Ethereum Uniswap V1 wallet").hex()[-40:]
ETH="0x"+"00"*20


def capture(url,artifact):
    snap=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    c=Capture(dict(snap,endpoint=url))
    at=c.selector
    head=rpc(url,"eth_getBlockByHash",[at["blockHash"],False])
    if rpc(url,"eth_chainId",[])!="0x1" or head["hash"]!=at["blockHash"]:
        raise ValueError("chain or parent")
    deadline=int(head["timestamp"],16)+300
    def view(to,sig,returns,types=(),values=()):
        return c.one(to,sig,types,values,[returns])[0]
    def role(address):
        code=bytes.fromhex(rpc(url,"eth_getCode",[address,at])[2:])
        if not code:raise ValueError("missing bytecode")
        return dict(address=address,bytes=len(code),runtime="0x"+keccak(code).hex())
    template=view(FACTORY,"exchangeTemplate()","address")
    count=view(FACTORY,"tokenCount()","uint256")
    deployed=runtime(c,artifact,[FACTORY,WETH])
    word=lambda a:"0x"+format(int(a,16),"064x")
    overrides={OWNER:{"balance":hex(100*10**18)},MODULE:{"code":deployed},
               EXEC:{"code":"0x"+RUNTIME.read_text().strip().removeprefix("0x"),"stateDiff":{"0x"+"00"*32:word(OWNER),word("1"):word(WETH)}}}
    if any(rpc(url,"eth_getCode",[address,at])!="0x" for address in overrides):
        raise ValueError("simulation address is occupied")
    def run(calls):
        return simulate_block(url,at,overrides,calls)
    def native_balance(a):
        return tx(MULTI,"getEthBalance(address)",["address"],[a])
    def holdings(a,token):
        return native_balance(a) if token==ETH else balance(token,a)
    def approve(t,address,n):
        return tx(t,"approve(address,uint256)",["address","uint256"],[address,n])
    markets={}
    for token in [STABLES[0],DAI,STABLES[1],WETH]:
        exchange=view(FACTORY,"getExchange(address)","address",["address"],[token])
        if view(FACTORY,"getToken(address)","address",["address"],[exchange])!=token or view(exchange,"tokenAddress()","address")!=token or view(exchange,"factoryAddress()","address")!=FACTORY:
            raise ValueError("registry reverse lookup")
        markets[token]=dict(role(exchange),token=token,native=str(view(MULTI,"getEthBalance(address)","uint256",["address"],[exchange])),
                            erc20=str(view(token,"balanceOf(address)","uint256",["address"],[exchange])))
    cases=[]
    def capture_leg(source,target,n,reference,mode):
        multi=mode in ("direct_pair","wallet_pair","v5_pair")
        pool=markets[source if multi or source not in (ETH,WETH) else target]["address"]
        target_pool=markets[target]["address"] if multi else None
        def fund():
            if source==ETH:return []
            if source==WETH:return [tx(WETH,"deposit()",value=n)]
            return [tx(FUNDING,"swapETHForExactTokens(uint256,address[],address,uint256)",
                       ["uint256","address[]","address","uint256"],[n,[WETH,source],OWNER,deadline],value=10**18)]
        prefix=fund()
        if source!=ETH:
            prefix.append(approve(source,EXEC if mode in ("v5","v5_pair") else MODULE if mode in ("wallet","wallet_pair","native_wallet") else pool,n))
        def order(out,end):
            if mode=="native_wallet":
                native_in=source==ETH
                return tx(MODULE,"executeNative(address,address,bool,uint256,uint256,uint256)",
                          ["address","address","bool"]+["uint256"]*3,[pool,target if native_in else source,native_in,n,out,end],value=n if native_in else 0)
            if mode=="direct" and source==ETH:
                return tx(pool,"ethToTokenSwapInput(uint256,uint256)",["uint256"]*2,[out,end],value=n)
            if mode=="direct":
                return tx(pool,"tokenToEthSwapInput(uint256,uint256,uint256)",["uint256"]*3,[n,out,end])
            if mode=="direct_pair":
                return tx(pool,"tokenToTokenSwapInput(uint256,uint256,uint256,uint256,address)",["uint256"]*4+["address"],[n,out,1,end,target])
            if mode=="wallet_pair":
                return tx(MODULE,"executePair(address,address,address,address,uint256,uint256,uint256)",["address"]*4+["uint256"]*3,[pool,target_pool,source,target,n,out,end])
            if mode=="wallet":
                return tx(MODULE,"execute(address,address,address,uint256,uint256,uint256)",["address"]*3+["uint256"]*3,[pool,source,target,n,out,end])
            step=(17,MODULE,source,target,10000,encode(["address"],[FACTORY if mode=="v5_pair" else pool]))
            return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",["address","uint256","address","uint256","uint256",STEP+"[]"],[source,n,target,out,end,[step]])
        cash=[holdings(a,t) for a in (OWNER,MODULE,EXEC) for t in (source,target)]
        rows=run(prefix+cash+[order(reference+1,deadline),order(reference,0),order(reference,deadline)]+cash)
        if any(v["status"]!="0x1" for v in rows[:len(prefix)]):
            raise ValueError(("independent funding failed",source,n,rows[0].get("error")))
        before,ops,after=rows[-15:-9],rows[-9:-6],rows[-6:]
        if any(x["status"]!="0x1" for x in before+after) or [x["status"] for x in ops[:2]]!=["0x0","0x0"]:
            raise ValueError(("failed price or expiry control",source,target,mode,reference))
        ab,cd=([int(v["returnData"],16) for v in group] for group in (before,after))
        rec=dict(mode=mode,source=source,target=target,pool=pool,target_pool=target_pool,input=str(n),calculated=str(reference))
        if ops[2]["status"]=="0x0":
            if ab!=cd:raise ValueError("revert changed balances")
            rec["revert"]=ops[2].get("error")
        else:
            if (ab[0]-cd[0],cd[1]-ab[1])!=(n,reference) or ab[2:]!=cd[2:]:
                raise ValueError(("wallet mismatch",source,target,mode,n,reference,ab,cd))
            rec.update(spent=str(n),received=str(reference),gas=int(ops[2]["gasUsed"],16),logs=ops[2]["logs"])
        cases.append(rec)
    for coin in [STABLES[0],DAI,STABLES[1]]:
        m=markets[coin];eth,erc20=int(m["native"]),int(m["erc20"]);pool=m["address"]
        for denominator in (1000,100):
            for buy in (True,False):
                n=(min(eth//denominator,10**15*1000//denominator) if buy else min(erc20//denominator,10**view(coin,"decimals()","uint8")*1000//denominator))
                if not n:continue
                ref=minimum(n,eth if buy else erc20,erc20 if buy else eth,3000)
                fn="getEthToTokenInputPrice" if buy else "getTokenToEthInputPrice"
                if view(pool,fn+"(uint256)","uint256",["uint256"],[n])!=ref:raise ValueError("official quote mismatch")
                if not ref:continue
                for mode in ("direct","native_wallet","wallet","v5"):
                    cash=ETH if mode in ("direct","native_wallet") else WETH
                    capture_leg(cash if buy else coin,coin if buy else cash,n,ref,mode)
    for source,target in [(DAI,STABLES[0]),(STABLES[0],DAI),(DAI,WETH),(WETH,DAI)]:
        first,last=markets[source],markets[target]
        n=min(int(first["erc20"])//50000,10**view(source,"decimals()","uint8"))
        via=minimum(n,int(first["erc20"]),int(first["native"]),3000)
        out=minimum(via,int(last["native"]),int(last["erc20"]),3000)
        if not out:continue
        for mode in ("direct_pair","wallet_pair","v5_pair"):
            capture_leg(source,target,n,out,mode)
    if not all(any(i["mode"]==m and "spent" in i for i in cases) for m in ("direct","native_wallet","wallet","v5","direct_pair","wallet_pair","v5_pair")):
        raise ValueError("uncovered payment form")
    return dict(selector=at,block=snap["block"],factory=role(FACTORY),template=role(template),count=count,markets=markets,module=MODULE,
                module_hash="0x"+keccak(hexstr=deployed).hex(),cases=cases)


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("artifact",type=Path)
    cli.add_argument("output",type=Path)
    args=cli.parse_args()
    for url in ["https://eth.drpc.org","https://rpc.mevblocker.io","https://ethereum-rpc.publicnode.com","https://ethereum.gateway.tenderly.co","https://eth-mainnet.public.blastapi.io"]:
        try:
            evidence=capture(url,json.loads(args.artifact.read_bytes()))
            break
        except (requests.RequestException,RuntimeError,ValueError,KeyError) as exc:
            print(url,str(exc)[:180],flush=True)
    else:
        raise RuntimeError("original Ethereum state unavailable")
    args.output.write_text(json.dumps(evidence,separators=(",",":")),encoding="utf8")
