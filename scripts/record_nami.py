"""Связывает опубликованные Nami размещения с реальными пулами Algebra и выплатами V5 на Base и Ethereum."""
import hashlib
import re
import sys

import requests
from eth_utils import keccak

from base_archive_endpoint import available
from record_hydrex_integral import EXEC, FOLDER, RUNTIME, VIRTUAL, VIRTUAL_FILE, read, rpc, simulate, map_ticks, write
from record_balancer_markets import Capture
from record_solidly_v3 import BASE_WETH, ETH_WETH, HOLDERS

CONFIG = "https://raw.githubusercontent.com/namifi/paraswap-dex-lib/f14c5093fc9e19ad9639b2b8bf9db9841a53f8af/src/dex/algebra-integral/config.ts"
BEACON = "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50"
USDC = {"base":"0x833589fcd6edb6e08f4c7c32d4f71b54bda02913","ethereum":"0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"}
BTC = {"base":"0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf","ethereum":"0x2260fac5e5542a773aa44fbcfedf7c193bc2c599"}


def capture(net):
    e=read(FOLDER/(net+".json"))
    if net=="base":e=available(e)
    c=Capture(e)
    raw=requests.get(CONFIG,timeout=20);raw.raise_for_status()
    key="BASE" if net=="base" else "MAINNET"
    part=raw.text.split("NamiV4: {",1)[1].split("[Network."+key+"]:",1)[1].split("},",1)[0]
    config={k:a.lower() for k,a in re.findall(r"(factory|quoter|router|stateMulticall): '(0x[0-9a-fA-F]{40})'",part)}
    if len(config)!=4:raise ValueError("Первичный перечень Nami изменился")
    factory,quoter=config["factory"],config["quoter"]
    wrapped=BASE_WETH if net=="base" else ETH_WETH
    header=rpc(e,"eth_getBlockByHash",[e["selector"]["blockHash"],False])
    dead=int(header["timestamp"],16)+90
    sources,contracts={},{}
    def hash_code(a):
        if a not in contracts:
            raw_code=rpc(e,"eth_getCode",[a,e["selector"]])
            if raw_code=="0x":raise ValueError("Нет кода "+a)
            code=bytes.fromhex(raw_code[2:])
            contracts[a]=dict(bytes=len(code),keccak="0x"+keccak(code).hex())
        return contracts[a]["keccak"]
    def match(a,name):
        if a in sources:return
        u=f"https://sourcify.dev/server/v2/contract/{e['chain_id']}/{a}"
        res=requests.get(u,params={"fields":"compilation,abi,sources,runtimeBytecode.onchainBytecode,runtimeBytecode.immutableReferences"},timeout=35)
        res.raise_for_status();d=res.json()
        buf=bytes.fromhex(d["runtimeBytecode"]["onchainBytecode"][2:])
        if d["runtimeMatch"] not in ("match","exact_match") or d["compilation"]["name"]!=name or "0x"+keccak(buf).hex()!=hash_code(a):
            raise ValueError("Неверный исходник "+a)
        source=d["compilation"]["fullyQualifiedName"].split(":")[0]
        sources[a]=dict(url=res.url,contract=name,path=source,sha256=hashlib.sha256(d["sources"][source]["content"].encode()).hexdigest(),
                        immutable=d["runtimeBytecode"].get("immutableReferences"),match=d["runtimeMatch"])
    def address(target,name,types=(),args=()):
        return c.one(target,name,types,args,["address"])[0]
    for a in config.values():hash_code(a)
    match(factory,"AlgebraFactory");match(quoter,"Quoter")
    registry=address(factory,"defaultPluginFactory()")
    deployer=address(factory,"poolDeployer()")
    if address(quoter,"factory()")!=factory or address(quoter,"poolDeployer()")!=deployer:
        raise ValueError("Квотер другой фабрики")
    if address(config["router"],"factory()")!=factory or address(config["router"],"poolDeployer()")!=deployer:
        raise ValueError("Роутер другой фабрики")
    state={EXEC:"0x"+RUNTIME.read_text().strip(),VIRTUAL:"0x"+VIRTUAL_FILE.read_text().strip()}
    for a in state:
        if rpc(e,"eth_getCode",[a,e["selector"]])!="0x":raise ValueError("Занят адрес симуляции")
    rows=[]
    for coin,sizes in [(wrapped,[10**14,10**15]),(BTC[net],[1000,10000])]:
        if coin not in HOLDERS[net]:continue
        usd=USDC[net]
        pool=address(factory,"poolByPair(address,address)",["address","address"],[coin,usd])
        if int(pool,16)==0:continue
        t0,t1=address(pool,"token0()"),address(pool,"token1()")
        if {t0,t1}!={coin,usd} or address(pool,"factory()")!=factory:raise ValueError("Чужой пул")
        pl=address(pool,"plugin()")
        if address(registry,"pluginByPool(address)",["address"],[pool])!=pl or address(pl,"pool()")!=pool:
            raise ValueError("Плагин не привязан к этому пулу")
        beacon="0x"+rpc(e,"eth_getStorageAt",[pl,BEACON,e["selector"]])[-40:]
        imp=address(beacon,"implementation()"); ext=address(pl,"extension()")
        if address(pl,"beacon()")!=beacon or address(pl,"pluginFactory()")!=registry:raise ValueError("Иной beacon")
        for a in (pool,registry,pl,beacon,imp,ext,deployer):hash_code(a)
        match(imp,"NamiPluginV1Diamond");match(ext,"NamiPluginV1Extension")
        code=bytearray.fromhex(rpc(e,"eth_getCode",[pl,e["selector"]])[2:])
        if bytes(code[75:107])!=int(pool,16).to_bytes(32):raise ValueError("Прокси не содержит пул по immutable offset")
        code[75:107]=bytes(32)
        gs=c.one(pool,"globalState()",returns=["uint160","int24","uint16","uint8","uint16","bool"])
        fees=[c.one(pl,"swapFee(bool)",["bool"],[side],["uint16"])[0] for side in (True,False)]
        mev_ex=address(pl,"getMevxExecutor()");mev_router=address(pl,"getMevxRouter()")
        if mev_ex==EXEC or mev_ex==quoter:raise ValueError("Квотер или V5 имеют особые условия MEV")
        row=dict(pool=pool,tokens=[t0,t1],plugin=pl,beacon=beacon,implementation=imp,extension=ext,
                 proxy_template="0x"+keccak(bytes(code)).hex(),mevx_executor=mev_ex,mevx_router=mev_router,
                 liquidity=str(c.one(pool,"liquidity()",returns=["uint128"])[0]),
                 state=[str(x) if isinstance(x,int) and not isinstance(x,bool) else x for x in gs],directional=fees,
                 cases=[])
        for a,b,amounts in ((coin,usd,sizes),(usd,coin,[10**6,10**7])):
            for size in amounts:
                r=simulate(e,pool,quoter,t0,a,b,size,state,dead,HOLDERS[net][a],True,wrapped)
                if r["override_fee"]!=fees[int(a!=t0)] or r["plugin_fee"]!=0:
                    raise ValueError("Действительная комиссия расходится с обычной стороной")
                row["cases"].append(r)
        row["tick_window"]=map_ticks(c,pool,[gs[1]]+[r["tick_after"] for r in row["cases"]])
        rows.append(row)
    if not rows:raise ValueError("Нет исследованных рынков")
    return dict(chain_id=e["chain_id"],endpoint=e["endpoint"],selector=e["selector"],primary=CONFIG,
                source_sha256=hashlib.sha256(raw.content).hexdigest(),config=config,plugin_factory=registry,
                pool_deployer=deployer,contracts=contracts,sources=sources,markets=rows,raw=c.calls,
                v5_runtime_keccak="0x"+keccak(bytes.fromhex(state[EXEC][2:])).hex())


if __name__=="__main__":
    network=sys.argv[1]
    write(FOLDER/("nami_"+network+".json"),capture(network))
