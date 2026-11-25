"""Independent continuous bound for the recorded SHOPINX path and BingX top ask.
For constant V3 liquidity through the tested interval, exact-in token0 -> token1
has the same real-valued swap formula as a CP with virtual reserves L/S and L*S.
Floors in integer pool execution can only lower the output.
"""
from decimal import Decimal as D, getcontext
import json, pathlib, sys
getcontext().prec=72
root=pathlib.Path(sys.argv[1]); result=pathlib.Path(sys.argv[2])
raw=json.loads((root/'tests/data/fit_volume_2026-11-15/responses.json').read_text())
r={x['name']:x['response']['result'][2:] for x in raw['results']}
at=lambda k,i:D(int(r[k][i*64:(i+1)*64],16))
scale=D(10)**18;f=D('.9999');g=D('.9975')
price_sqrt=at('SHOPINX-WBNB pcs_v3 slot0',0)/(D(2)**96)
liq=at('SHOPINX-WBNB pcs_v3 liquidity',0)/scale
x=liq/price_sqrt;y=liq*price_sqrt
u=at('WBNB-USDT v2 getReserves',0)/scale;w=at('WBNB-USDT v2 getReserves',1)/scale
# Yield before integer round-down is A*n/(B+C*n) for n tokens actually sold.
A=u*g*f*y;B=w*x;C=f*(w+g*y)
top=json.loads((root/'tests/data/bingx_minqty_2026-11-15/book_ticker_SHOPINX-USDT.json').read_text())['data'][0]
a=D(top['askPrice']);b=D(top['bidPrice']);cap=D(top['askVolume']);buy_keep=D('.999');withdraw=D(1)
gas=(D(2)*D('.00012')+D('.000021'))*D('598.53')
rows=[]
for sigma in [D(0),D('.0001')]:
 keep=(-D('2.5')*sigma*D(60).sqrt()).exp()
 # With closure disabled the continuous pnl is concave. Solve F'(n)=0.
 n=((keep*A*B*buy_keep/a).sqrt()-B)/C
 gross=max(D(1)/buy_keep,min(cap,(n+withdraw)/buy_keep))
 net=max(D(0),gross*buy_keep-withdraw)
 pnl=keep*A*net/(B+C*net)-a*gross-gas
 rows.append(dict(sigma=str(sigma),upper_qty=str(gross),upper_after_fixture_costs_usdt=str(pnl),fixed_costs_usdt=str(gas)))
print(json.dumps(dict(type='continuous_bound_top_cex_to_dex',spot_sell_upper=str(A/B),spot_buy_lower=str((u/w)*price_sqrt**2/(f*g)),ask=str(a),bid=str(b),rows=rows)),file=result.open('a'))
