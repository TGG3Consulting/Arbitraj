"""Архив Mint Club V2: реестр Bond, кривые ERC20 и полные расчёты обеих сторон на одном блоке."""
import argparse
import json
import time
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import OWNER, calldata, call, simulate_block


BOND = "0xc5a076cad94176c2996b32d8466be1ce757faa27"
BOND_HASH = "0xdd4f11b3651371f72233223557a47b279c9aa7be6dbc415da7a9cd9dc858285c"
TOKEN = "aa70bc79fd1cb4a6fba717018351f0c3c64b79df"
MULTI = "0xca11bde05977b3631167028862be2a173976ca11"
NETWORK = {
    "base": (8453, 62262756, "0xdf458884e3b25319497f122bfe50a2dff59b0079421fec1e1a9ac403d3c3e715",
             "0x4200000000000000000000000000000000000006", ["https://mainnet.base.org","https://base.drpc.org"]),
    "ethereum": (1, 26494988, "0x2372196a2961b307333be48d3dad74a2e723fd459c2cb01e225a0a97f50e7a7a",
                 "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2", ["https://mainnet.gateway.tenderly.co","https://eth.drpc.org"]),
}


def unpack(data):
    return bytes.fromhex(data.removeprefix("0x"))


def leg(target, name, types=(), values=(), value=0):
    return call(target, calldata(name, types, values), value)


class Archive:
    def __init__(self, network):
        chain, self.height, block, self.weth, self.endpoints = NETWORK[network]
        self.session = requests.Session()
        self.tag = {"blockHash":block,"requireCanonical":True}
        self.chain = chain
        self.header = self.rpc("eth_getBlockByHash",[block,False])
        if int(self.header["number"],16)!=self.height or self.code_hash(BOND)!=BOND_HASH:
            raise ValueError("unverified Bond deployment")
        if int(self.rpc("eth_chainId",[]),16)!=chain:
            raise ValueError("wrong chain")

    def rpc(self, method, params):
        errors=[]
        for attempt in range(4):
            for url in self.endpoints:
                try:
                    reply=self.session.post(url,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=35)
                    reply.raise_for_status()
                    data=reply.json()
                    if "error" in data: raise ValueError(str(data["error"])[:250])
                    return data["result"]
                except (requests.RequestException,ValueError,KeyError) as err:
                    errors.append(str(err)[:250])
            time.sleep(2**attempt)
        raise RuntimeError((method,errors[-len(self.endpoints):]))

    def code(self, address):
        return unpack(self.rpc("eth_getCode",[address,self.tag]))

    def code_hash(self, address):
        return "0x"+keccak(self.code(address)).hex()

    def view(self, target, name, types=(), values=(), out=("uint256",)):
        data=calldata(name,types,values)
        result=self.rpc("eth_call",[{"to":target,"data":data},self.tag])
        return decode(out,unpack(result))

    def many(self, target, name, argtypes, values, out):
        result=[]
        for start in range(0,len(values),128):
            calls=[(target,False,unpack(calldata(name,argtypes,[x]))) for x in values[start:start+128]]
            raw,=self.view(MULTI,"aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[calls],["(bool,bytes)[]"])
            if len(raw)!=len(calls) or not all(ok for ok,_ in raw):raise ValueError("incomplete factory page")
            result.extend(decode(out,data) for _,data in raw)
        return result

    def simulate(self, seq):
        return simulate_block(None,self.tag,{OWNER:{"balance":hex(10**20)}},seq,read_rpc=lambda _,m,p:self.rpc(m,p))


def curve(steps, supply, size, mint, rate, unit):
    if size<=0 or (mint and supply+size>steps[-1][0]) or (not mint and size>supply):
        raise ValueError("size outside curve")
    start,end=(supply,supply+size) if mint else (supply-size,supply)
    reserve=0
    low=0
    for high,price in steps:
        span=max(0,min(end,high)-max(start,low))
        subtotal=span*price
        reserve+=(subtotal+unit-1)//unit if mint else subtotal//unit
        low=high
    royalty=reserve*rate//10000
    return (reserve+royalty if mint else reserve-royalty),royalty


def wallet(arc, token, amount, cost, mint_royalty, refund, burn_royalty):
    balance=lambda t,owner:leg(t,"balanceOf(address)",["address"],[owner])
    approve=lambda t,n:leg(t,"approve(address,uint256)",["address","uint256"],[BOND,n])
    mint=lambda ceiling:leg(BOND,"mint(address,uint256,uint256,address)",["address","uint256","uint256","address"],[token,amount,ceiling,OWNER])
    burn=lambda floor:leg(BOND,"burn(address,uint256,uint256,address)",["address","uint256","uint256","address"],[token,amount,floor,OWNER])
    # One parent and one sequence: issuer accounting, Bond's physical inventory, and both wallet balances.
    balances=[balance(arc.weth,OWNER),balance(token,OWNER),balance(arc.weth,BOND),
              leg(BOND,"tokenBond(address)",["address"],[token])]
    seq=[leg(arc.weth,"deposit()",value=cost),approve(arc.weth,cost)]
    def mark(calls):
        idx=len(seq)
        seq.extend(calls)
        return idx
    before=mark(balances)
    refuse_mint=mark([mint(cost-1)])
    rolled_mint=mark(balances)
    buy=mark([mint(cost)])
    minted=mark(balances)
    quote=mark([leg(BOND,"getRefundForTokens(address,uint256)",["address","uint256"],[token,amount])])
    mark([approve(token,amount)])
    refuse_burn=mark([burn(refund+1)])
    rolled_burn=mark(balances)
    sell=mark([burn(refund)])
    burned=mark(balances)
    r=arc.simulate(seq)
    if [i for i,row in enumerate(r) if row["status"]!="0x1"]!=[refuse_mint,refuse_burn]:
        raise ValueError(("unexpected wallet result",token,[(i,row.get("error")) for i,row in enumerate(r) if row["status"]!="0x1"]))
    def snapshot(idx):
        wallet_reserve,wallet_token,physical=(int(r[idx+n]["returnData"],16) for n in range(3))
        terms=decode(["address","uint16","uint16","uint40","address","uint256"],unpack(r[idx+3]["returnData"]))
        if terms[4]!=arc.weth:raise ValueError("reserve changed")
        return (wallet_reserve,wallet_token,physical,terms[5])
    old,issued,final=map(snapshot,(before,minted,burned))
    if old!=snapshot(rolled_mint) or issued!=snapshot(rolled_burn):
        raise ValueError("rejected limit changed balances")
    if tuple(b-a for a,b in zip(old,issued,strict=True))!=(-cost,amount,cost,cost-mint_royalty):
        raise ValueError("mint delta disagrees with Bond accounting")
    if tuple(b-a for a,b in zip(issued,final,strict=True))!=(refund,-amount,-refund,-refund-burn_royalty):
        raise ValueError("burn delta disagrees with Bond accounting")
    if decode(["uint256","uint256"],unpack(r[quote]["returnData"]))!=(refund,burn_royalty):
        raise ValueError("post-mint refund and royalty disagree with model")
    return dict(size=str(amount),mint_cost=str(cost),redeem=str(refund),mint_gas=int(r[buy]["gasUsed"],16),
                burn_gas=int(r[sell]["gasUsed"],16),mint_reject=r[refuse_mint]["error"],burn_reject=r[refuse_burn]["error"],
                mint_gross=str(cost-mint_royalty),mint_royalty=str(mint_royalty),burn_gross=str(refund+burn_royalty),burn_royalty=str(burn_royalty),
                payer=OWNER,before=list(map(str,old)),after_mint=list(map(str,issued)),after_burn=list(map(str,final)),
                mint_logs=r[buy]["logs"],burn_logs=r[sell]["logs"])


def capture(network):
    arc=Archive(network)
    count,=arc.view(BOND,"tokenCount()")
    tokens=[row[0] for row in arc.many(BOND,"tokens(uint256)",["uint256"],list(range(count)),["address"])]
    if len(set(tokens))!=count:raise ValueError("duplicate registry token")
    terms=arc.many(BOND,"tokenBond(address)",["address"],tokens,["address","uint16","uint16","uint40","address","uint256"])
    directory=[dict(index=i,token=t,reserve=row[4],bonded=str(row[5]),mint_bps=row[1],burn_bps=row[2]) for i,(t,row) in enumerate(zip(tokens,terms,strict=True))]
    poolset=[r for r in directory if r["reserve"]==arc.weth and int(r["bonded"])>0]
    poolset.sort(key=lambda row:int(row["bonded"]),reverse=True)
    clone=bytes.fromhex("363d3d373d3d3d363d73"+TOKEN+"5af43d82803e903d91602b57fd5bf3")
    markets=[]
    for row in poolset:
        token=row["token"]
        if arc.code(token)!=clone:continue
        if arc.view(token,"bond()",out=["address"])[0]!=BOND:raise ValueError("token bound to another bond")
        steps,=arc.view(BOND,"getSteps(address)",["address"],[token],["(uint128,uint128)[]"])
        supply,=arc.view(token,"totalSupply()")
        unit=10**arc.view(token,"decimals()")[0]
        room=steps[-1][0]-supply
        # Текущий участок и переход следующей ступени, когда его можно оплатить собственным ETH в архиве.
        sizes={min(room,max(1,unit//100)),min(room,unit)}
        next_step=next((end for end,_ in steps if end>supply),None)
        if next_step:sizes.add(min(room,next_step-supply+1))
        trials=[]
        for size in sorted(sizes):
            if size<=0:continue
            cost,fee=curve(steps,supply,size,True,row["mint_bps"],unit)
            if not 0<cost<10**18:continue
            q=arc.view(BOND,"getReserveForToken(address,uint256)",["address","uint256"],[token,size],["uint256","uint256"])
            if q!=(cost,fee):raise ValueError(("mint rounding",row,size,q,cost,fee))
            refund,burn_fee=curve(steps,supply+size,size,False,row["burn_bps"],unit)
            if refund:trials.append(wallet(arc,token,size,cost,fee,refund,burn_fee))
        if trials:
            markets.append(dict(**row,supply=str(supply),steps=[[str(h),str(p)] for h,p in steps],trials=trials))
        if len(markets)==3:break
    if not markets:raise ValueError("no ERC20/WETH wallet controls covered")
    return dict(chain_id=arc.chain,height=arc.height,selector=arc.tag,state_root=arc.header["stateRoot"],bond=BOND,
                bond_hash=BOND_HASH,token_implementation="0x"+TOKEN,weth=arc.weth,weth_runtime_hash=arc.code_hash(arc.weth),total=count,directory=directory,markets=markets)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("network",choices=NETWORK)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    args.output.write_text(json.dumps(capture(args.network),separators=(",",":"))+"\n",encoding="utf8",newline="\n")
