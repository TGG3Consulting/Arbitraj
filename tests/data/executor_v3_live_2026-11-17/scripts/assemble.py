import sys; sys.path.insert(0,"/tmp/claude-1000/ex/run")
from lib2 import *
import collections,hashlib,os
OUT="/data/handoff-from-old/out/tests/data/executor_v3_live_2026-11-17"
r=json.load(open("results.json")); D=json.load(open("cands.json")); S=r["summary"]; C=r["calls"]
bypid={p["id"]:p for p in D["all"]}
Z="0x"+"0"*40
def unwrap(rv):
    # WrappedError(address,bytes4,bytes,bytes) 0x90bfb865
    if rv and "0x90bfb865" in rv:
        h=rv[rv.index("0x90bfb865"):]
        hook="0x"+h[10+24:10+64]; sel_="0x"+h[10+64:10+72]
        reason=""
        i=h.find("08c379a0")
        if i>0:
            try:
                ln=int(h[i+8+64:i+8+128],16); reason="Error("+bytes.fromhex(h[i+8+128:i+8+128+2*ln]).decode("utf8","replace")+")"
            except Exception: reason="?"
        return "WrappedError(hook=%s, hookSelector=%s, reason=%s)"%(hook,sel_,reason or "нет текста")
    return rv
SW={"v4":topic('Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)'),"cl":topic('Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24,uint16)'),"bin":topic('Swap(bytes32,address,int128,int128,uint24,uint24,uint16)')}
def credit_of(p,logs,tout):
    for l in logs:
        if l["address"].lower()==(V4PM if p["fam"]=="v4" else p["pm"]) and l["topics"][0]==SW[p["fam"]] and l["topics"][1]==p["id"]:
            idx=0 if tout==p["key"]["c0"] else 1; return abs(sw(l["data"],idx))
for s,c in zip(S,C):
    s["revert_raw"]=s.get("revert"); 
    if s.get("revert"): s["revert"]=unwrap(s["revert"])
    c["revert"]=s.get("revert")
    if s["status"] and s["case"]=="c":
        cr=s["swap_credit"]["out_side"]; s["pool_part_credit"]=str(cr); s["got_eq_v2_formula_plus_swap_credit"]=(int(s["v2_expected"])+abs(cr)==int(s["got"]))
        s.pop("got_eq_swap_credit",None)
    if s["status"] and s["case"]=="g_cycle":
        pb=bypid[s["second_pool_id"]]; cr=credit_of(pb,C[S.index(s)]["logs"],USDT)
        s["second_step_swap_credit"]=str(cr); s["got_eq_swap_credit"]=(cr==int(s["got"]))
        s["first_step_swap_credit"]=str(credit_of(bypid[s["pool_id"]],C[S.index(s)]["logs"],s["coin"]))
S2=[dict(s) for s in S]
# hooks_reject
HR=[]
for s in S:
    if s.get("hooks") not in (None,Z) and not s["status"]:
        HR.append(dict(pool_id=s["pool_id"],hooks=s["hooks"],revert=s["revert"],case=s["case"],revert_raw=s["revert_raw"]))
# нет пулов
n_v4h=[p for p in D["all"] if p["fam"]=="v4" and p["key"]["hooks"]!=Z]
NO=[dict(case="нет пулов",kind=19,query="Uniswap V4 с крючком, USDT в паре, ликвидность>0 на H",result=dict(H=r["H"],searched="Initialize менеджера %s, окно логов узла ~595000 блоков до H0=134171376 (узел хранит ~3 суток, лимит getLogs 5000 блоков), topics[2]=USDT и topics[3]=USDT"%V4PM,pools_with_usdt_total=sum(1 for p in D["all"] if p["fam"]=="v4"),with_hook=len(n_v4h),with_hook_liquidity_gt0=sum(1 for p in n_v4h if p.get("liq_ok")),hook_pools=[dict(pool_id=p["id"],hooks=p["key"]["hooks"],liquidity=p["liq"].get("liquidity")) for p in n_v4h],verdict="вид 19 С КРЮЧКОМ — НЕПРОВЕРЕН (все пулы с крючком и USDT имеют ликвидность 0)")),
    dict(case="нет пулов",kind=21,query="Infinity Bin, USDT в паре, достаточная ликвидность на H",result=dict(H=r["H"],searched="Initialize BinPoolManager %s, окно ~595000 блоков"%BINPM,found=sum(1 for p in D["all"] if p["fam"]=="bin"),pool=[dict(pool_id=p["id"],active_id=p["liq"].get("activeId"),bin_reserveX=p["liq"].get("bin_reserveX"),bin_reserveY=p["liq"].get("bin_reserveY"),quoter_err=(p.get("q20_err") or "")[:200]) for p in D["all"] if p["fam"]=="bin"],verdict="вид 21 — НЕПРОВЕРЕН: единственный пул с USDT содержит пыль (резерв активного бина 1/1); симуляция откатилась (см. случай a, kind 21)")),
    dict(case="нет пулов",kind=20,query="Infinity Stable (вид 20, ключ с крючком из dex.stable_hooks)",result=dict(H=r["H"],searched="настройка dex.stable_hooks в /api/settings",found=0,verdict="Infinity Stable — НЕПРОВЕРЕН: dex.stable_hooks не задана (по описанию настройки — «DEX не подключена»), адресов крючков для поиска нет"))]
json.dump(S2+NO,open(OUT+"/summary.json","w"),ensure_ascii=False,indent=1)
json.dump(HR,open(OUT+"/hooks_reject.json","w"),ensure_ascii=False,indent=1)
calls=[dict(c,case=s["case"],kind=s.get("kind")) for s,c in zip(S,C)]
json.dump(calls,open(OUT+"/calls.json","w"),ensure_ascii=False)
raw=json.load(open("phase1_rpc.json"))+json.load(open("main_rpc_a.json"))+json.load(open("main_rpc_b.json"))
json.dump(raw,open(OUT+"/raw_rpc.json","w"))
json.dump(dict(slots=r["slots"],coins=r["coin_slots"]),open(OUT+"/slots.json","w"),indent=1)
# stats for README
st=collections.Counter(); 
for s in S: st[(s["case"],s.get("kind"),s["status"],(s.get("hooks") not in (None,Z)))]+=1
for k,v in sorted(st.items(),key=lambda x:str(x[0])): print(k,v)
print("raw",len(raw),os.path.getsize(OUT+"/raw_rpc.json"))
dev=[(s["case"],s["kind"],s["pool_id"][:8],s.get("diff_vs_quoter_pct"),s.get("diff_vs_quoter_after_a_pct")) for s in S if s["status"]]
for d in dev: print(d)
print([ (s["case"],s["kind"],s["pool_id"][:8],s.get("got_eq_swap_credit"),s.get("got_eq_v2_formula_plus_swap_credit")) for s in S if s["status"] and s["case"] in("c","g_cycle")])
print([ (s["pool_id"][:8],s["revert"][:200]) for s in S if not s["status"]])
