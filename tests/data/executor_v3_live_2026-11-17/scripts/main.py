import sys; sys.path.insert(0,"/tmp/claude-1000/ex/run")
from lib2 import *
import os,collections,datetime
OUT="/data/handoff-from-old/out/tests/data/executor_v3_live_2026-11-17"
os.makedirs(OUT,exist_ok=True)
n=Node(8545)
P1=json.load(open("phase1.json"))
BIN_RT=open("/tmp/claude-1000/ex/ArbExecutor.bin-runtime").read().strip()
assert BIN_RT.startswith("0x") or True
if not BIN_RT.startswith("0x"): BIN_RT="0x"+BIN_RT
H=h2i(n.rpc("eth_blockNumber",[])["result"])
hb=n.rpc("eth_getBlockByNumber",[hex(H),False])["result"]
HT=h2i(hb["timestamp"]); DL=HT+600
print("H",H,"time",HT,"deadline",DL,flush=True)
X="0x00000000000000000000000000000000a4b0e0c3"
assert call(n,X,"0x",H) in (None,"0x") 
code=n.rpc("eth_getCode",[X,hex(H)])["result"]; bx=n.rpc("eth_getBalance",[X,hex(H)])["result"]
assert code=="0x" and h2i(bx)==0,(code,bx)
# ---- pools from events
def t(x,i): return "0x"+x["topics"][i][26:]
pools=[]
for fam,evs in P1["events"].items():
    for e in evs:
        d=e["data"]; pid=e["topics"][1]; c0=t(e,2);c1=t(e,3)
        if int(c0,16)==0: continue
        if fam=="v4":
            ts=sw(d,1); ts=ts if True else ts
            ts24=word(d,1)&0xffffff; ts=ts24-(1<<24) if ts24>>23 else ts24
            k=dict(c0=c0,c1=c1,fee=word(d,0),ts=ts,hooks=adr(d,2)); kb=key_v4(k); pm=V4PM
        else:
            k=dict(c0=c0,c1=c1,hooks=adr(d,0),pm=(CLPM if fam=="cl" else BINPM),fee=word(d,1),params="0x"+d[2+64*2:2+64*3]); kb=key_inf(k); pm=k["pm"]
        ok=pid_of(kb)==pid
        pools.append(dict(fam=fam,id=pid,key=k,keybytes=kb,hash_ok=ok,pm=pm,block=h2i(e["blockNumber"])))
print("events pools",collections.Counter(p["fam"] for p in pools),"hash_ok",collections.Counter((p["fam"],p["hash_ok"]) for p in pools),flush=True)
pools=[p for p in pools if p["hash_ok"]]
seen=set();up=[]
for p in pools:
    if p["id"] in seen: continue
    seen.add(p["id"]); up.append(p)
pools=up
# ---- infinity: poolIdToPoolKey check on chain
for p in pools:
    if p["fam"] in("cl","bin"):
        r=call(n,p["pm"],"0x"+sel("poolIdToPoolKey(bytes32)")+p["id"][2:],H)
        if r and len(r)>=2+64*6:
            kb="0x"+r[2:2+64*6]; p["onchain_key_ok"]=(pid_of(kb)==p["id"]) and kb==p["keybytes"]
        else: p["onchain_key_ok"]=None
    else:
        p["onchain_key_ok"]=None
# ---- liquidity, zf, quoter
def keyargs(p):
    return p["keybytes"][2:]
def qdata(p,zf,amt,hook="0x"):
    if p["fam"]=="v4": s="quoteExactInputSingle(((address,address,uint24,int24,address),bool,uint128,bytes))"; hd=8
    else: s="quoteExactInputSingle(((address,address,address,address,uint24,bytes32),bool,uint128,bytes))"; hd=9
    hd=len(p["keybytes"][2:])//64+3
    return "0x"+sel(s)+w(32)+keyargs(p)+w(1 if zf else 0)+w(amt)+w(32*hd)+w(0)
Q={"v4":V4Q,"cl":CLQ,"bin":BINQ}
def quote(p,zf,amt):
    r,e=call_err(n,Q[p["fam"]],qdata(p,zf,amt),H,W)
    if r and len(r)>=66: return word(r,0),None
    return None,e or r
def liq(p):
    f=p["fam"]
    if f=="v4":
        base=h2i("0x"+keccak(bytes.fromhex(p["id"][2:]+w(6))).hex())
        s0=call(n,V4PM,"0x1e2eaeaf"+w(base),H); l=call(n,V4PM,"0x1e2eaeaf"+w(base+3),H)
        return dict(sqrtP=(word(s0,0)&((1<<160)-1)) if s0 else None, liquidity=word(l,0)&((1<<128)-1) if l else None)
    if f=="cl":
        l=call(n,CLPM,"0x"+sel("getLiquidity(bytes32)")+p["id"][2:],H); s0=call(n,CLPM,"0xc815641c"+p["id"][2:],H)
        return dict(sqrtP=word(s0,0) if s0 else None,liquidity=word(l,0) if l else None)
    s0=call(n,BINPM,"0xc815641c"+p["id"][2:],H)
    if not s0: return dict(activeId=None)
    act=word(s0,0); b=call(n,BINPM,"0x4580c3c5"+p["id"][2:]+w(act),H)
    return dict(activeId=act,bin_reserveX=word(b,0) if b else None,bin_reserveY=word(b,1) if b else None)
cands=[]
t0=time.time()
for p in pools:
    zf=(p["key"]["c0"]==USDT)
    p["zf_usdt_in"]=zf; p["coin"]=p["key"]["c1"] if zf else p["key"]["c0"]
    if p["coin"]==USDT: continue
    p["liq"]=liq(p)
    ok_liq=(p["liq"].get("liquidity") or 0)>0 if p["fam"]!="bin" else ((p["liq"].get("bin_reserveX") or 0)+(p["liq"].get("bin_reserveY") or 0))>0
    p["liq_ok"]=ok_liq
    if not ok_liq: continue
    o20,e=quote(p,zf,20*10**18); p["q20"]=o20; p["q20_err"]=e
    if o20: 
        p["q2"],_=quote(p,zf,2*10**18)
        cands.append(p)
print("pools",len(pools),"liq_ok+quote",collections.Counter(p["fam"] for p in cands),"t",round(time.time()-t0),flush=True)
json.dump(dict(H=H,cands=[{k:v for k,v in p.items()} for p in cands],all=[{k:v for k,v in p.items()} for p in pools]),open("cands.json","w"))
json.dump(n.log,open("main_rpc_a.json","w"))
