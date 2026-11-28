"""Проверяет все положительные продажи в USDC: средства получаем собственной предшествующей покупкой."""
from fractions import Fraction
from eth_abi import decode
from record_base_slipstream_executor import OWNER,call
from record_raindex_quotes import ORDER,TAKE,data
from raindex_wire import take

USDC="0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
LIMIT="(address,address,uint256,uint256,uint256,uint256)"


def value(word):
    raw=bytes.fromhex(word[2:])
    return int.from_bytes(raw[4:],"big",signed=True)*Fraction(10)**int.from_bytes(raw[:4],"big",signed=True)


def comparison(src,module,tag,stamp,seed,run,bal,approve,tx,end,identities):
    book=src["book"]
    offers=[(e,q) for e in src["entries"] for q in e["quotes"] if q.get("live")
            and value(q["cap"])>0 and value(q["ratio"])>0]
    def terms(e,q):
        o=decode([ORDER],bytes.fromhex(e["order"][2:]))[0]
        query=call(book,data("quote2",[TAKE],[(o,q["i"],q["j"],[])]))
        query["from"]=module
        rows=run([query,tx(q["pay"],"decimals",[],[]),tx(q["receive"],"decimals",[],[])])
        live,cp,r=decode(["bool","bytes32","bytes32"],bytes.fromhex(rows[0]["returnData"][2:]))
        cap,ratio="0x"+cp.hex(),"0x"+r.hex()
        di,do=(int(v["returnData"],16) for v in rows[1:])
        return dict(order=e["hash"],i=q["i"],j=q["j"],pay=q["pay"],receive=q["receive"],
                    decimals=[di,do],live=live,cap=cap,ratio=ratio)
    def attempt(prefix,e,t,receive_fixed,size,lo,hi,floor,protected):
        di,do=t["decimals"]; fr,to=t["pay"],t["receive"]; dest=module if protected else book
        action=take(6,e["order"],t["i"],t["j"],size,size,do if receive_fixed else di,
                    receive_fixed,bytes.fromhex(t["ratio"][2:]))
        cash=(tx(module,"fill",[LIMIT,"bytes"],[(fr,to,lo,hi,floor,end),bytes.fromhex(action[2:])])
              if protected else call(book,action,gas=10_000_000))
        prefix=prefix+[approve(fr,dest,hi)]
        checks=[bal(fr),bal(to),bal(fr,module),bal(to,module),tx(fr,"allowance",["address","address"],[module,book])]
        rows=run(prefix+checks+[cash]+checks)
        pos=len(prefix)+len(checks)
        if any(r["status"]!="0x1" for r in rows[:pos]+rows[pos+1:]):
            raise ValueError("funding or measurement")
        op=rows[pos]
        balance=lambda rows:[str(int(r["returnData"],16)) for r in rows]
        before,after=balance(rows[len(prefix):pos]),balance(rows[pos+1:])
        result=dict(order=t["order"],fixedReceive=receive_fixed,quantity=str(size),fromBalance=before[0],
                    call=cash,protected=protected,status=op["status"],before=before,after=after,
                    logs=op["logs"],error=op.get("error"),gas=int(op["gasUsed"],16))
        if op["status"]=="0x1":
            paid=int(before[0])-int(after[0]);got=int(after[1])-int(before[1])
            if not (lo<=paid<=hi and got>=floor and before[2:]==after[2:]):
                raise ValueError("net payment")
            result.update(paid=str(paid),net=str(got))
            return prefix+[cash],result
        if before!=after:
            raise ValueError("failed transfer")
        return None,result
    ceil=lambda v:-(-v.numerator//v.denominator)
    sales=[];purchases={}
    for e,q in offers:
        if q["receive"]!=USDC:
            continue
        t=terms(e,q); token=t["pay"]; di,do=t["decimals"]
        if token not in purchases:
            funding=[];earned=0;chain=None
            for source,sq in offers:
                if (sq["pay"],sq["receive"])!=(USDC,token):
                    continue
                ft=terms(source,sq)
                cap=int(value(ft["cap"])*10**di)
                for size in sorted({cap,max(1,cap//2)},reverse=True):
                    maxpay=ceil(value(ft["ratio"])*size*Fraction(10**do,10**di))
                    prefix,fact=attempt(seed,source,ft,True,size,1,maxpay,size,True)
                    funding.append(fact)
                    if prefix and size>earned:
                        earned,chain=size,prefix
                    if prefix:
                        break
                    ordinary_prefix,ordinary=attempt(seed,source,ft,True,size,1,maxpay,size,False)
                    funding.append(ordinary)
                    if ordinary_prefix:
                        if size>earned:
                            earned,chain=size,ordinary_prefix
                        break
            purchases[token]=(funding,earned,chain)
        _,earned,chain=purchases[token];tests=[]
        if chain:
            ratio=value(t["ratio"])*Fraction(10**di,10**do)
            cap=int(value(t["cap"])*10**do)
            limit=min(earned,ceil(cap*ratio))
            for size in sorted({limit,max(1,limit-1),max(1,limit//2)},reverse=True):
                output=int(size/ratio)
                fixed=output>cap and ceil(cap*ratio)==size
                target=cap if fixed else size
                for protected in (True,False):
                    _,observed=attempt(chain,e,t,fixed,target,size,size,1,protected)
                    tests.append(observed)
        sales.append(dict(quote=t,available=str(earned),tests=tests))
    return dict(chain=8453,height=src["height"],selector=tag,stamp=stamp,payer=OWNER,book=book,
                modules=identities,sales=sales,purchases={k:v[0] for k,v in purchases.items()})
