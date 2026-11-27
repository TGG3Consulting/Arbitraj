"""Canonical role and implementation identities for read-only previews of individual service quotes."""
import gzip
import json
import requests
from eth_abi import encode,decode
from eth_utils import keccak
from record_llama_top_contracts import DATA,rpc,sig

def capture():
    result={}
    holder='0x0000000000001ff3684f28c67538d4d072c22734'
    registry='0x00000000000004533fe15556b1e086bb1a72ceae'
    settled=json.loads(gzip.decompress((DATA/'kipseli_base_settled.json.gz').read_bytes()))
    for network in ('base','ethereum'):
        anchors=json.loads((DATA/f'{network}.json').read_bytes())
        at=anchors['selector']
        role_addresses={'allowance_holder':holder,'settler_registry':registry}
        if network=='base':
            role_addresses.update(kipseli_router=settled['router'],source=settled['swap_impl'],forward=settled['intermediary'],cash=settled['cash'])
        def code(a):
            raw=bytes.fromhex(rpc(network,'eth_getCode',[a,at])[2:])
            if not raw:raise ValueError('No public runtime')
            return dict(address=a,bytes=len(raw),runtime='0x'+keccak(raw).hex())
        roles={k:code(a) for k,a in role_addresses.items()}
        reg=roles['settler_registry']
        for name in ('ownerOf(uint256)','prev(uint128)'):
            call={'to':registry,'data':sig(name)[:10]+encode(['uint256'],[2]).hex()}
            raw=rpc(network,'eth_call',[call,at]);reg[name]=decode(['address'],bytes.fromhex(raw[2:]))[0]
        endpoint='https://base.gateway.tenderly.co' if network=='base' else 'https://eth.drpc.org'
        slot='0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc'
        args=dict(jsonrpc='2.0',id=1,method='eth_getStorageAt',params=[registry,slot,at])
        response=requests.post(endpoint,json=args,timeout=15);response.raise_for_status()
        impl=code('0x'+response.json()['result'][-40:])
        reg.update(implementation=impl['address'],implementation_hash=impl['runtime'])
        result[network]=dict(selector=at,height=anchors['block'],roles=roles)
    return result

if __name__=='__main__':
    (DATA/'keyed_quote_endpoints.json').write_text(json.dumps(capture(),indent=2)+'\n',encoding='utf8')
