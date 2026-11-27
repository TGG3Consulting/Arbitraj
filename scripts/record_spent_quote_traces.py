"""Read-only traces of the original finalized payments, including contract-wallet signature acceptance."""
import gzip
import json
import requests
from eth_abi import decode,encode
from record_llama_top_contracts import DATA,sig

def trace(network,tx,receipt):
    url='https://base.drpc.org' if network=='base' else 'https://eth.drpc.org'
    response=requests.post(url,json=dict(jsonrpc='2.0',id=1,method='trace_transaction',params=[tx]),timeout=30)
    response.raise_for_status()
    rows=response.json()['result']
    if any(r['transactionHash']!=tx or r['blockHash']!=receipt['blockHash'] for r in rows):raise ValueError('Different inclusion')
    return [r for r in rows if r.get('type')=='call' and not r.get('error')]

def build():
    base=json.load(gzip.open(DATA/'kipseli_base_settled.json.gz'))
    maker=json.load(gzip.open(DATA/'llama_top_0x_rfq.json.gz'))
    calls=trace('base',base['transaction']['hash'],base['receipt'])
    matches=[r for r in calls if r['action']['to']==base['router'] and r['action']['from']==base['user'] and r['action']['input']==base['settled_action']]
    if len(matches)!=1 or int(matches[0]['result']['output'],16)!=int(base['received']):raise ValueError('Different Kipseli execution')
    calls=trace('ethereum',maker['transaction']['hash'],maker['receipt'])
    rfq='RFQ(address,((address,uint256),uint256,uint256),address,bytes,address,uint256)'
    types=['address','((address,uint256),uint256,uint256)','address','bytes','address','uint256']
    sel=bytes.fromhex(sig(rfq)[2:10])
    actions=[c for c in calls if c['action']['to']==maker['settler'] and sel in bytes.fromhex(c['action']['input'][2:])]
    if len(actions)!=1:raise ValueError('Ambiguous Settler entry')
    body=bytes.fromhex(actions[0]['action']['input'][2:])
    if body.count(sel)!=1:raise ValueError('Ambiguous RFQ action')
    recipient,permit,signer,signature,token,maximum=decode(types,body.split(sel)[1])
    (output,output_limit),nonce,deadline=permit
    if (recipient,signer,token,output)!=(maker['settler'],maker['maker'],maker['maker_input']['address'],maker['maker_output']['address']):
        raise ValueError('Maker, recipient or tokens differ from cash transfers')
    checks=[r for r in calls if r['traceAddress'][:len(actions[0]['traceAddress'])]==actions[0]['traceAddress']
            and r['action']['to']==signer and r['action']['input'][:10]=='0x1626ba7e' and bytes.fromhex(r['result']['output'][2:])==bytes.fromhex('1626ba7e')+bytes(28)]
    if len(checks)!=1:raise ValueError('Missing original ERC-1271 acceptance')
    digest,signed=decode(['bytes32','bytes'],bytes.fromhex(checks[0]['action']['input'][10:]))
    paid=int(maker['maker_input']['data'],16);got=int(maker['maker_output']['data'],16)
    if signature!=signed or not (0<paid<=maximum and 0<got<=output_limit):raise ValueError('Signed amounts or signature differ')
    return dict(kipseli={'frame':matches[0]},zeroex=dict(entry=actions[0],signature_check=checks[0],digest='0x'+digest.hex(),nonce=str(nonce),
                expiry=deadline,signed_input_limit=str(maximum),signed_output_limit=str(output_limit),paid=str(paid),received=str(got),
                settled_action='0x'+(sel+encode(types,[recipient,permit,signer,signature,token,maximum])).hex()))
if __name__=='__main__':
    (DATA/'kipseli_zeroex_settled_traces.json.gz').write_bytes(gzip.compress(json.dumps(build(),separators=(',',':')).encode(),mtime=0))
