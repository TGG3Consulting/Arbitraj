"""Closed-form reference, restricted to a single CEX level and two unchanged V3-liquidity
segments. The CI validates the proposed quantity against the independent native full quote."""
from decimal import Decimal as D, getcontext
from pathlib import Path
import json
getcontext().prec=70
def hint(c, src):
 j=json.loads(Path(src).read_text());raw={r['name']:r['response']['result'][2:] for r in j['results']}
 tags=list(dict.fromkeys(k.split()[0] for k in raw if k.endswith(' slot0')))
 def pool(tag, word):
  val=lambda k,i:int(raw[tag+' '+k][64*i:64*i+64],16)
  signed=lambda n:n-(1<<256) if n>>255 else n
  s=D(val('slot0',0))/(2**96);tick=signed(val('slot0',1));L=D(val('liquidity',0))/D(10)**18;fee=1-D(val('fee',0))/D(10)**6
  sp=val('tickSpacing',0);ticks=[(word-6)*256*sp,((word+7)*256-1)*sp]
  for nm, contents in raw.items():
   if not nm.startswith(tag+' TickLens word '):continue
   n=int(contents[64:128],16)
   for i in range(n):
    start=128+192*i;ticks.append(signed(int(contents[start:start+64],16)))
  down=max(t for t in ticks if t<=tick);up=min(t for t in ticks if t>tick)
  # At exact initialized current tick the active side must be checked by sqrtP.
  return L/s,L*s,s,L,fee,down,up
 a=pool(tags[0],c['words'][0]);b=pool(tags[1],c['words'][1]); x,y,S,L,f,tl,tu=a; u,w,T,Z,g,t2l,t2u=b
 q0=D(c['k_qty']);keep=D(c['risk_keep']);fee=D('.001');withdraw=D(c['withdraw']); step=D('.000001')
 A=u*y*f*g; B=w*x; C=f*(w+g*y);M=u*y;K=g*w*f*x;J=g*(w*f+y)
 rev=c['reverse'];book=c['bids' if rev else 'asks'];px,depth=map(D,book[0])
 if rev:
  marginal=(keep-fee)*px
  q=(K-(M*K/marginal).sqrt())/J
  # An exactly sufficient CEX depth reserve can span more than the first bid; h probed natively.
  q=max(D(0),min(q,depth))
  cost=M*q/(K-J*q);wb=y*q/(f*(x-q))
  snew=S*x/(x-q);tnew=T*(1-wb/w)
  ok1=snew <= D('1.0001')**(D(tu)/2);ok2=tnew>=D('1.0001')**(D(t2l)/2)
 else:
  net=((keep*A*B*(1-fee)/px).sqrt()-B)/C
  q=max(D(0),min((net+withdraw)/(1-fee),depth));n=max(D(0),q*(1-fee)-withdraw)
  wb=y*f*n/(x+f*n);cost=q*px
  snew=S*x/(x+f*n);tnew=T*(1+wb*g/w)
  ok1=snew>=D('1.0001')**(D(tl)/2);ok2=tnew<=D('1.0001')**(D(t2u)/2)
 return {'qty':str((q/step).to_integral_value(rounding='ROUND_FLOOR')*step),'inside_shop_segment':ok1,'inside_bridge_segment':ok2,'inside_level':q<=depth,'inside_budget':cost*D(c['spend_mult'])<=D(c['budget']), 'ticks':[[tl,tu],[t2l,t2u]],'width':6,'first_level_size':str(depth)}
