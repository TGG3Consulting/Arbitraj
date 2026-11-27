"""Epsilon Base V6. Сравнивает реально выплаченные суммы через те же V3 рынки и проверяет границы исходного кошелька."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import OWNER, ROOT, call, calldata as data, rpc

ROUTER = "0x303ca5c65aabcb1ce242df93f478c41e0e4d2580"
PROVIDERS = ("0xb20c411fc84fbb27e78608c24d0056d974ea9411", "0x2626664c2603336e57b271c5c0b26f421741e481")
WETH = "0x4200000000000000000000000000000000000006"
USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
UINT = "uint256"
SINGLE = "(address,address,uint24,address,uint256,uint256,uint160)"
ORDER = "(address,address,uint256,uint256,uint256,uint16,uint24,uint24,uint256)"
ROUTES = "(address[],bytes[],uint256[])"
PERMIT = "((address,uint256),uint256,uint256)"
ZERO = "0x" + "00"*20
WRAPPER = "0x" + keccak(text="Epsilon own terminal archive Base")[-20:].hex()


def method(to, sig, types=(), args=(), value=0):
    return call(to, data(sig, types, args), value, gas=6000000)


def bytes_call(sig, types, args):
    return bytes.fromhex(data(sig, types, args)[2:])


def leg(a, b, tier, recipient, amount, floor, deadline):
    inner = bytes_call("exactInputSingle("+SINGLE+")", [SINGLE], [(a,b,tier,recipient,amount,floor,0)])
    return bytes_call("multicall(uint256,bytes[])", [UINT,"bytes[]"], [deadline,[inner]])


def read(url, params):
    r=requests.post(url,json={"jsonrpc":"2.0","id":1,"method":"eth_call","params":params},timeout=20)
    r.raise_for_status()
    ans=r.json()
    if 'result' not in ans:
        raise ValueError(str(ans.get('error'))[:180])
    return bytes.fromhex(ans['result'][2:])


def record(url, artifact):
    anchor=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())["selector"]
    head=rpc(url,"eth_getBlockByHash",[anchor["blockHash"],False])
    if int(rpc(url,"eth_chainId",[]),16)!=8453 or head["hash"]!=anchor["blockHash"]:
        raise ValueError("chain or canonical parent")
    for address in (OWNER,WRAPPER):
        if rpc(url,"eth_getCode",[address,anchor])!="0x":
            raise ValueError("experimental address already has code")
    def view(to, sig, types, args, kind):
        return decode([kind],read(url,[method(to,sig,types,args),anchor]))[0]
    fee=view(ROUTER,"protocolFeeBps()",[],[],"uint16")
    paused=view(ROUTER,"isPaused()",[],[],"bool")
    if paused or fee!=5:
        raise ValueError("unexpected venue state on agreed historical block")
    roles=[]
    tiers=[]
    for i,p in enumerate(PROVIDERS):
        factory=view(p,"factory()",[],[],"address")
        if not view(ROUTER,"allowedTargets(address)",["address"],[p],"bool"):
            raise ValueError("provider not accepted")
        pools={}
        for tier in ((100,200,300,400,750,3000,10000) if i==0 else (100,500,3000,10000)):
            pool=view(factory,"getPool(address,address,uint24)",["address","address","uint24"],[WETH,USDC,tier],"address")
            if int(pool,16):
                pools[tier]={"pool":pool,"liquidity":str(view(pool,"liquidity()",[],[],"uint128"))}
        active=max(pools,key=lambda t:int(pools[t]["liquidity"]))
        if not int(pools[active]["liquidity"]):raise ValueError("no live pool")
        tiers.append(active)
        roles.append(dict(router=p,factory=factory,tier=active,pools=pools,code=keccak(hexstr=rpc(url,"eth_getCode",[p,anchor])).hex()))
    state={OWNER:{"balance":hex(10**18)}}
    creation=artifact["creation"].removeprefix("0x")+encode(["address"]*3,[ROUTER,*PROVIDERS]).hex()
    runtime=read(url,[{"from":OWNER,"data":"0x"+creation},anchor,state])
    template=bytes.fromhex(artifact["runtime"].removeprefix("0x"))
    masked=bytearray(runtime)
    for copies in artifact["immutableReferences"].values():
        values=set()
        for ref in copies:
            start,size=ref["start"],ref["length"]
            values.add(runtime[start:start+size]);masked[start:start+size]=template[start:start+size]
        if len(values)!=1:raise ValueError("immutable copies")
    if masked!=template:raise ValueError("wallet runtime mismatch")
    state[WRAPPER]={"code":"0x"+runtime.hex()}
    stamp=int(head["timestamp"],16)+2
    def sim(calls):
        [b]=rpc(url,"eth_simulateV1",[{"blockStateCalls":[{"blockOverrides":{"time":hex(stamp)},"stateOverrides":state,"calls":calls}]},anchor])
        if b["parentHash"]!=anchor["blockHash"] or len(b["calls"])!=len(calls):raise ValueError("simulation anchor")
        return b["calls"]
    def approval(a,spender,qty):
        return method(a,"approve(address,uint256)",["address",UINT],[spender,qty])
    def funded(a):
        n=2*10**16
        steps=[method(WETH,"deposit()",value=n)]
        if a==USDC:
            steps.extend([approval(WETH,PROVIDERS[1],n),call(PROVIDERS[1],"0x"+leg(WETH,USDC,tiers[1],OWNER,n,1,stamp+60).hex())])
        return steps
    def trial(a,b,qty,share,mode,minimum,deadline):
        net=qty-qty*fee//10000
        budget=qty if mode=='provider' else net
        portions=(budget*share//100,budget-budget*share//100)
        active=[i for i,n in enumerate(portions) if n]
        dst=WRAPPER if mode=='guarded' else ROUTER if mode=='epsilon' else PROVIDERS[active[0]]
        if mode=='provider':
            if len(active)!=1:raise ValueError("direct baseline must have one provider")
            i=active[0];payload=leg(a,b,tiers[i],OWNER,qty,minimum,deadline)
        elif mode=='epsilon':
            paths=[leg(a,b,tiers[i],ROUTER,portions[i],0,deadline) for i in active]
            routed=([PROVIDERS[i] for i in active],paths,[portions[i] for i in active])
            types=["address","address",UINT,UINT,UINT,"bool",ROUTES,PERMIT,"bytes"]
            payload=bytes_call("swap("+",".join(types)+")",types,[a,b,qty,minimum,minimum,False,routed,((ZERO,0),0,0),b""])
        else:
            payload=bytes_call("trade("+ORDER+")",[ORDER],[(a,b,qty,minimum,deadline,fee,*tiers,portions[0])])
        prefix=funded(a)+[approval(a,dst,qty)]
        watches=[method(token,"balanceOf(address)",["address"],[who]) for who in (OWNER,ROUTER,WRAPPER) for token in (a,b)]
        watches.extend(method(a,"allowance(address,address)",["address","address"],[ROUTER,p]) for p in PROVIDERS)
        watches.append(method(a,"allowance(address,address)",["address","address"],[WRAPPER,ROUTER]))
        reads=sim(prefix+watches+[call(dst,"0x"+payload.hex(),gas=6000000)]+watches)
        idx=len(prefix)+len(watches); op=reads[idx]
        if any(x["status"]!="0x1" for x in reads[:idx]+reads[idx+1:]):raise ValueError("funding or observation")
        before=[int(x["returnData"],16) for x in reads[len(prefix):idx]];after=[int(x["returnData"],16) for x in reads[idx+1:]]
        if before[2:]!=after[2:]:raise ValueError("venue or module inventory changed")
        return dict(spent=str(before[0]-after[0]),received=str(after[1]-before[1]),before=list(map(str,before)),after=list(map(str,after)),
                    status=op['status'],error=op.get('error'),gas=int(op['gasUsed'],16),approveGas=int(reads[len(prefix)-1]['gasUsed'],16),
                    target=dst,calldata="0x"+payload.hex(),logs=op.get("logs", []))
    cases=[]
    for a,b,sizes in ((WETH,USDC,(10**15,5*10**15)),(USDC,WETH,(10**6,5*10**6))):
        for n in sizes:
            for share in (0,50,100):
                for mode in (('epsilon','guarded') if share==50 else ('provider','epsilon','guarded')):
                    loose=trial(a,b,n,share,mode,1,stamp+60)
                    if loose['status']!='0x1':raise ValueError((a,b,n,share,mode,loose['error']))
                    net=int(loose['received'])
                    ok=trial(a,b,n,share,mode,net,stamp+60)
                    controls=[]
                    for floor,end in [(net+1,stamp+60),(net,stamp-1)]:
                        fail=trial(a,b,n,share,mode,floor,end)
                        if (fail['status'],fail['spent'],fail['received'])!=('0x0','0','0'):raise ValueError("accepted above net/expired")
                        controls.append(fail['error'])
                    if (ok['status'],ok['spent'],int(ok['received']))!=('0x1',str(n),net):raise ValueError("debit or exact floor")
                    cases.append(dict(input=a,output=b,amount=str(n),firstPercent=share,mode=mode,controls=controls,**ok))
    return dict(chain=8453,at=anchor,height=int(head['number'],16),timestamp=stamp,router=ROUTER,routerHash="0x"+keccak(hexstr=rpc(url,'eth_getCode',[ROUTER,anchor])).hex(),owner=OWNER,wallet=WRAPPER,walletHash="0x"+keccak(runtime).hex(),feeBps=fee,providers=roles,cases=cases)


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument('artifact',type=Path);parser.add_argument('out',type=Path);args=parser.parse_args()
    errors=[]
    for endpoint in ("https://mainnet.base.org","https://base.gateway.tenderly.co","https://base.drpc.org"):
        try:
            result=record(endpoint,json.loads(args.artifact.read_bytes()))
            args.out.write_text(json.dumps(result,separators=(',',':')),'utf8');break
        except (ValueError,KeyError,requests.RequestException,RuntimeError) as err:
            errors.append((endpoint,str(err)[:300]))
    else:raise ValueError(errors)

