import sys; sys.path.insert(0,"/tmp/claude-1000/ex/run")
from lib2 import *
import os,collections
OUT="/data/handoff-from-old/out/tests/data/executor_v3_live_2026-11-17"
os.makedirs(OUT,exist_ok=True)
n=Node(8545)
D=json.load(open("cands.json"))
BIN_RT=open("/tmp/claude-1000/ex/ArbExecutor.bin-runtime").read().strip()
if not BIN_RT.startswith("0x"): BIN_RT="0x"+BIN_RT
import hashlib
SHA=hashlib.sha256(open("/tmp/claude-1000/ex/ArbExecutor.bin-runtime","rb").read()).hexdigest()
assert SHA=="c8885a75f7318513c6c7303012e56bcc651ea5def431edb98b2111221bf537c5"
H=h2i(n.rpc("eth_blockNumber",[])["result"])
HT=h2i(n.rpc("eth_getBlockByNumber",[hex(H),False])["result"]["timestamp"]); DL=HT+600
print("H",H,HT,DL,flush=True)
X="0x00000000000000000000000000000000a4b0e0c3"
assert n.rpc("eth_getCode",[X,hex(H)])["result"]=="0x" and h2i(n.rpc("eth_getBalance",[X,hex(H)])["result"])==0
Z="0x"+"0"*40
AMT=20*10**18
def qdata(p,zf,amt):
    s=("quoteExactInputSingle(((address,address,uint24,int24,address),bool,uint128,bytes))" if p["fam"]=="v4" else "quoteExactInputSingle(((address,address,address,address,uint24,bytes32),bool,uint128,bytes))")
    hd=len(p["keybytes"][2:])//64+3
    return "0x"+sel(s)+w(32)+p["keybytes"][2:]+w(1 if zf else 0)+w(amt)+w(32*hd)+w(0)
QA={"v4":V4Q,"cl":CLQ,"bin":BINQ}
def quote(p,zf,amt):
    r,e=call_err(n,QA[p["fam"]],qdata(p,zf,amt),H,W)
    return (word(r,0),None) if r and len(r)>=66 else (None,e or r)
KIND={"v4":19,"cl":20,"bin":21}; MGR={"v4":V4PM,"cl":VAULT,"bin":VAULT}
def step(p,tin,tout,share=10000): return dict(kind=KIND[p["fam"]],pool=MGR[p["fam"]],tokenIn=tin,tokenOut=tout,shareBps=share,data=p["keybytes"])
# ---------- re-quote at current H
Zd=lambda p:p["key"]["hooks"]!=Z
def refresh(p):
    zf=p["key"]["c0"]==USDT
    p["q20"],p["q20_err"]=quote(p,zf,AMT)
    p["q2"],_=quote(p,zf,2*10**18)
    return p
# depth ranking
def impact(p):
    if not p.get("q20") or not p.get("q2"): return 0
    return p["q20"]/(10*p["q2"])
allp=D["all"]
v4n=[p for p in D["cands"] if p["fam"]=="v4" and not Zd(p)]
cln=[p for p in D["cands"] if p["fam"]=="cl" and not Zd(p)]
clh=[p for p in D["all"] if p["fam"]=="cl" and Zd(p) and p.get("liq_ok")]
v4h=[p for p in D["all"] if p["fam"]=="v4" and Zd(p)]
bins=[p for p in D["all"] if p["fam"]=="bin"]
# V2 pair per coin (top by depth)
def v2pair(coin):
    r=call(n,V2F,"0x"+sel("getPair(address,address)")+ad(USDT)+ad(coin),H)
    if not r: return None
    pr=adr(r,0)
    if int(pr,16)==0: return None
    res=call(n,pr,"0x0902f1ac",H); t0=call(n,pr,"0x0dfe1681",H)
    if not res or not t0: return None
    r0,r1=word(res,0),word(res,1); isu0=(adr(t0,0)==USDT)
    ru,rc=(r0,r1) if isu0 else (r1,r0)
    return dict(pair=pr,reserve_usdt=ru,reserve_coin=rc,usdt_is_token0=isu0)
for lst in (v4n,cln):
    lst.sort(key=lambda p:-impact(p))
top_v4=v4n[:40]; top_cl=cln[:40]
v2={}
for p in top_v4+top_cl:
    c=p["coin"]
    if c not in v2: v2[c]=v2pair(c)
good_v2={c for c,v in v2.items() if v and v["reserve_usdt"]>=500*10**18}
def pick(lst,k,want=lambda p:True):
    out=[]
    for p in lst:
        if impact(p)>0.9 and p["coin"] not in [o["coin"] for o in out] and want(p): out.append(p)
        if len(out)>=k: break
    return out
sel_v4=pick(top_v4,2,lambda p:p["coin"] in good_v2)
sel_v4+=pick([p for p in top_v4 if p not in sel_v4],3-len(sel_v4))
sel_cl=pick(top_cl,2,lambda p:p["coin"] in good_v2)
sel_cl+=pick([p for p in top_cl if p not in sel_cl],3-len(sel_cl))
# cycle coin: in both v4 and cl
both=[c for c in {p["coin"] for p in top_v4}&{p["coin"] for p in top_cl}]
cyc=[]
for c in both:
    a=max([p for p in top_v4 if p["coin"]==c],key=impact); b=max([p for p in top_cl if p["coin"]==c],key=impact)
    if impact(a)>0.9 and impact(b)>0.9: cyc.append((min(impact(a),impact(b)),a,b))
cyc.sort(key=lambda x:-x[0]); cyc=cyc[:2]
hk_cl=sorted(clh,key=lambda p:-(p["liq"].get("liquidity") or 0))[:9]
sel_hk_v4=[]  # hooked v4 pools have zero liquidity (liq_ok false) -> none
print("sel v4",[(p["id"][:10],p["coin"][:8],round(impact(p),3)) for p in sel_v4],flush=True)
print("sel cl",[(p["id"][:10],p["coin"][:8],round(impact(p),3)) for p in sel_cl],flush=True)
print("hooked cl",[(p["id"][:10],p["key"]["hooks"][:10]) for p in hk_cl],flush=True)
print("cycles",[(p[1]["id"][:10],p[2]["id"][:10],p[1]["coin"][:10]) for p in cyc],flush=True)
print("v2 good coins",len(good_v2),flush=True)
json.dump(dict(sel_v4=[p["id"] for p in sel_v4],sel_cl=[p["id"] for p in sel_cl],hk_cl=[p["id"] for p in hk_cl],cyc=[(a["id"],b["id"]) for _,a,b in cyc],v2=v2),open("sel.json","w"))
# ---------- slots
def sim_ok(r): return "result" in r
def find_allow_slot(tok):
    # simulate: spender Xi differs per i, slot override per i; check allowance(W,Xi)==MAX
    ov={tok:{"stateDiff":{}}}; calls=[]; sp=[]
    cand=list(range(61))+[0x52c63247e1f47db19d5ce0460030c497f067ca4cebf71ba98eeadabe20bace00+1]
    for j,i in enumerate(cand):
        s="0x"+("%040x"%(0xA110000+j))
        sp.append(s); ov[tok]["stateDiff"][allowslot(W,s,i)]=padv(MAXU)
        calls.append({"from":W,"to":tok,"input":"0xdd62ed3e"+ad(W)+ad(s)})
    r=sim(n,H,ov,calls)
    if "result" not in r: return None,"sim error "+json.dumps(r.get("error"))[:200]
    cs=r["result"][0]["calls"]
    hits=[i for i,c in enumerate(cs) if c.get("status")=="0x1" and c.get("returnData","0x")!="0x" and word(c["returnData"],0)==MAXU]
    return (cand[hits[0]] if len(hits)==1 else None),("hits=%s"%[cand[h] for h in hits])
def find_bal_slot_spec(tok,holder):
    b=bal(n,tok,holder,H)
    for i in range(21):
        if stor(n,tok,mapslot(holder,i),H)==b and b: return i
    return None
slots={}
holder=None
for hh in (VAULT,V4PM,CLPM):
    if (bal(n,USDT,hh,H) or 0)>0: holder=hh;break
ub=find_bal_slot_spec(USDT,holder)
ua,ua_note=find_allow_slot(USDT)
curW=bal(n,USDT,W,H)
slots["USDT"]=dict(balance_slot=ub,balance_holder_used=holder,allowance_slot=ua,allowance_method="simulate allowance(W,Xi)==2^256-1 for 21 spenders",allowance_note=ua_note,W_usdt_balance_H=curW)
print("USDT slots",slots["USDT"],flush=True)
if ub is None or ua is None:
    print("слоты не найдены (USDT)"); json.dump(slots,open("slots.json","w")); sys.exit(1)
coin_slots={}
def coin_slot(c):
    if c not in coin_slots:
        a,note=find_allow_slot(c); coin_slots[c]=dict(allowance_slot=a,note=note)
    return coin_slots[c]["allowance_slot"]
def overrides(coins):
    pass
def overrides(coins):
    ov={X:{"code":BIN_RT,"stateDiff":{hexslot(0):padv(int(W,16)),hexslot(1):padv(int(WBNB,16))}},
        USDT:{"stateDiff":{mapslot(W,ub):padv(curW+AMT),allowslot(W,X,ua):padv(MAXU)}}}
    for c in coins:
        s=coin_slot(c)
        if s is None: return None
        ov.setdefault(c,{"stateDiff":{}})["stateDiff"][allowslot(W,X,s)]=padv(MAXU)
    return ov
json.dump(dict(slots=slots,coins=coin_slots),open("slots_tmp.json","w"))
# ---------- run helpers
SWAPT={"v4":topic('Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)'),
       "cl":topic('Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24,uint16)'),
       "bin":topic('Swap(bytes32,address,int128,int128,uint24,uint24,uint16)')}
SIMS=[]
def balcalls(coin): return [{"from":W,"to":USDT,"input":"0x70a08231"+ad(X)},{"from":W,"to":coin,"input":"0x70a08231"+ad(X)}]
def run(ov,calls):
    r=sim(n,H,ov,calls)
    if "result" not in r: return None,r.get("error")
    return r["result"][0]["calls"],None
def parse_call(c):
    st=1 if c.get("status")=="0x1" else 0
    rd=c.get("returnData","0x")
    return dict(status=st,returnData=rd,gasUsed=h2i(c.get("gasUsed","0x0")),logs=c.get("logs",[]),revert=("" if st else decode_revert(rd,c.get("error"))),error=c.get("error"))
def credit(p,logs,tout):
    for l in logs:
        if l["address"].lower()==(V4PM if p["fam"]=="v4" else p["pm"]) and l["topics"][0]==SWAPT[p["fam"]] and l["topics"][1]==p["id"]:
            d=l["data"]; idx=0 if tout==p["key"]["c0"] else 1
            return dict(amount0=sw(d,0),amount1=sw(d,1),out_side=sw(d,idx))
    return None
summary=[];calls_out=[];hooks_reject=[]
def rec(case,p,tin,tout,amt,res,xl,quoter_out,extra=None):
    got=int(res["returnData"],16) if res["status"] and len(res["returnData"])>=66 else None
    cr=credit(p,res["logs"],tout) if p else None
    d=dict(case=case,kind=KIND[p["fam"]] if p else None,pool_id=p["id"] if p else None,manager=MGR[p["fam"]] if p else None,pool_manager=(V4PM if p["fam"]=="v4" else p["pm"]) if p else None,hooks=p["key"]["hooks"] if p else None,token_in=tin,token_out=tout,amount_in=str(amt),got=str(got) if got is not None else None,quoter_out=str(quoter_out) if quoter_out is not None else None,ur_out=None,gas=res["gasUsed"],status=res["status"],revert=res["revert"],x_left=xl)
    d["swap_credit"]=cr
    if got is not None and quoter_out:
        d["diff_vs_quoter_pct"]=round(abs(got-quoter_out)*100/quoter_out,6)
    if got is not None and cr: d["got_eq_swap_credit"]=(abs(cr["out_side"])==got)
    if extra: d.update(extra)
    summary.append(d)
    calls_out.append(dict(case=case,pool_id=d["pool_id"],status=res["status"],returnData=res["returnData"],gasUsed=res["gasUsed"],logs=res["logs"],revert=res["revert"],error=res["error"],x_left=xl))
    if p and p["key"]["hooks"]!=Z and not res["status"]:
        hooks_reject.append(dict(pool_id=p["id"],hooks=p["key"]["hooks"],revert=res["revert"],case=case))
    return d
def xleft(c3,c4): 
    return dict(usdt=str(word(c3.get("returnData","0x0"),0)) if c3.get("returnData","0x")!="0x" else None,coin=str(word(c4.get("returnData","0x0"),0)) if c4.get("returnData","0x")!="0x" else None)
def case_ab(p):
    coin=p["coin"]; zf=p["key"]["c0"]==USDT
    ov=overrides([coin])
    if ov is None: summary.append(dict(case="a",pool_id=p["id"],status=0,revert="слоты не найдены (монета)",coin=coin)); return
    qo,qe=quote(p,zf,AMT)
    stA=[step(p,USDT,coin)]
    ex=enc_execute(USDT,AMT,coin,1,DL,stA)
    cs,err=run(ov,[{"from":W,"to":X,"input":ex}]+balcalls(coin))
    if cs is None: print("sim err",err); return
    ra=parse_call(cs[0]); xl=xleft(cs[1],cs[2])
    d=rec("a",p,USDT,coin,AMT,ra,xl,qo,dict(quoter_err=qe,coin=coin))
    print("a",KIND[p["fam"]],p["id"][:10],"status",ra["status"],"got",d.get("got"),"q",qo,"diff",d.get("diff_vs_quoter_pct"),ra["revert"][:80],flush=True)
    if not ra["status"]: return
    M=int(ra["returnData"],16)
    qs,qse=quote(p,not zf,M)   # quote at H for sell
    sell=enc_execute(coin,M,USDT,1,DL,[step(p,coin,USDT)])
    qafter={"from":W,"to":QA[p["fam"]],"input":qdata(p,not zf,M)}
    cs2,err2=run(ov,[{"from":W,"to":X,"input":ex}]+balcalls(coin)+[qafter,{"from":W,"to":X,"input":sell}]+balcalls(coin))
    if cs2 is None: print("sim2 err",err2); return
    ra2=parse_call(cs2[0]); qaf=cs2[3]; rb=parse_call(cs2[4]); xl2=xleft(cs2[5],cs2[6])
    qa=word(qaf["returnData"],0) if qaf.get("status")=="0x1" and len(qaf.get("returnData","0x"))>=66 else None
    d2=rec("b",p,coin,USDT,M,rb,xl2,qs,dict(quoter_out_after_a=str(qa) if qa else None,a_rerun_got=str(int(ra2["returnData"],16)) if ra2["status"] else None,a_rerun_same=(ra2["returnData"]==ra["returnData"]),quoter_err=qse))
    if qa and rb["status"]: d2["diff_vs_quoter_after_a_pct"]=round(abs(int(rb["returnData"],16)-qa)*100/qa,6)
    print("b",KIND[p["fam"]],p["id"][:10],"status",rb["status"],"got",d2.get("got"),"q",qs,"q_after",qa,"diff",d2.get("diff_vs_quoter_pct"),d2.get("diff_vs_quoter_after_a_pct"),rb["revert"][:80],flush=True)
def v2out(inn,rin,rout,fee_pips=2500): 
    a=inn*(10**6-fee_pips); return a*rout//(rin*10**6+a)
def case_c(p):
    coin=p["coin"]; v=v2.get(coin)
    if not v or v["reserve_usdt"]<500*10**18: return False
    zf=p["key"]["c0"]==USDT
    ov=overrides([coin])
    if ov is None: return False
    half=AMT//2
    v2o=v2out(half,v["reserve_usdt"],v["reserve_coin"]); qo,qe=quote(p,zf,half)
    stp=[dict(kind=0,pool=v["pair"],tokenIn=USDT,tokenOut=coin,shareBps=5000,data="0x"+w(2500)),step(p,USDT,coin)]
    ex=enc_execute(USDT,AMT,coin,1,DL,stp)
    cs,err=run(ov,[{"from":W,"to":X,"input":ex}]+balcalls(coin))
    if cs is None: print("sim err",err); return False
    r=parse_call(cs[0]); xl=xleft(cs[1],cs[2])
    exp=(v2o+qo) if qo else None
    d=rec("c",p,USDT,coin,AMT,r,xl,exp,dict(v2_pair=v["pair"],v2_expected=str(v2o),pool_quoter_half=str(qo),note="quoter_out = V2 по формуле (0,25%) на половину + квотер DEX на вторую половину"))
    print("c",KIND[p["fam"]],p["id"][:10],"status",r["status"],"got",d.get("got"),"exp",exp,"diff",d.get("diff_vs_quoter_pct"),r["revert"][:80],flush=True)
    return True
def case_g(pa,pb):
    coin=pa["coin"]; ov=overrides([coin])
    if ov is None: return
    zfa=pa["key"]["c0"]==USDT
    o1,_=quote(pa,zfa,AMT); o2,_=quote(pb,not (pb["key"]["c0"]==USDT),o1) if o1 else (None,None)
    ex=enc_execute(USDT,AMT,USDT,1,DL,[step(pa,USDT,coin),step(pb,coin,USDT)])
    cs,err=run(ov,[{"from":W,"to":X,"input":ex}]+balcalls(coin))
    if cs is None: print("sim err",err); return
    r=parse_call(cs[0]); xl=xleft(cs[1],cs[2])
    d=rec("g_cycle",pa,USDT,USDT,AMT,r,xl,o2,dict(second_pool_id=pb["id"],second_kind=KIND[pb["fam"]],second_hooks=pb["key"]["hooks"],first_quote=str(o1),coin=coin))
    print("g",KIND[pa["fam"]],"->",KIND[pb["fam"]],pa["id"][:8],pb["id"][:8],"status",r["status"],"got",d.get("got"),"q",o2,"diff",d.get("diff_vs_quoter_pct"),r["revert"][:80],flush=True)
# ---------- execute everything
for p in sel_v4+sel_cl: refresh(p)
for p in hk_cl: refresh(p)
done_split={}
for p in sel_v4+sel_cl: case_ab(p)
for p in sel_v4[:3]+sel_cl[:3]:
    if case_c(p): done_split[p["fam"]]=done_split.get(p["fam"],0)+1
for p in hk_cl: 
    p["q20"],p["q20_err"]=quote(p,p["key"]["c0"]==USDT,AMT); case_ab(p)
for _,a,b in cyc: refresh(a);refresh(b);case_g(a,b);case_g(b,a)
for p in bins: 
    p["q20"],p["q20_err"]=quote(p,p["key"]["c0"]==USDT,AMT); case_ab(p)
json.dump(dict(H=H,HT=HT,deadline=DL,X=X,W=W,summary=summary,slots=slots,coin_slots=coin_slots,hooks_reject=hooks_reject,calls=calls_out),open("results.json","w"))
json.dump(n.log,open("main_rpc_b.json","w"))
print("DONE",len(summary),flush=True)
