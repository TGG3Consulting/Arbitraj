import test from 'node:test';
import assert from 'node:assert/strict';
import {deploymentScope} from '../src/lib/deploymentScope.js';
const profile = (chainId,id) => ({
  id, venue:chainId===1?'eth_dex':'base_dex', kind:'carbon_v1',
  key:'carbon-create:'+chainId, settings:{chainId,market:'controller',hashes:['runtime']},
});
const eth=profile(1,'carbon-ethereum'), base=profile(8453,'alien-carbon-base');
const intent={chain:1,nonce:'0x5',from:'owner',creation:'code'};
function setup() {
  const rows=new Map(), selected={value:structuredClone(eth)};
  const storage={getItem:key=>rows.get(key)??null,setItem:(key,v)=>rows.set(key,v),removeItem:key=>rows.delete(key)};
  const open=()=>deploymentScope(storage,()=>selected.value);
  return {open,selected,rows};
}
test('a selection change before wallet dispatch cannot reserve a nonce or assign a router',async()=>{
  for (const next of [base,{...eth,id:'second-profile'}, {...eth,venue:'another_venue'},
    {...eth,router:'configured'}, {...eth,settings:{...eth.settings,market:'changed'}}, false]) {
    const {open,selected,rows}=setup(), action=open(), writes=[];
    const verify=Promise.resolve().then(()=>action.install('verified',(...args)=>writes.push(args)));
    selected.value=next;
    assert.throws(()=>action.beforeOrAfterSend(intent));
    await assert.rejects(verify);
    assert.equal(rows.size,0);
    assert.deepEqual(writes,[]);
  }
});
test('a wallet hash, rejection or lost reply belongs to its original deployment journal',async()=>{
  for(const response of ['hash','refused','unknown']) {
    const {open,selected,rows}=setup(), action=open();
    action.beforeOrAfterSend(intent);
    selected.value=base;
    const other={chain:8453,nonce:'0x21',from:'another',creation:'other-code'};
    open().beforeOrAfterSend(other);
    await Promise.resolve();
    if(response==='hash') action.beforeOrAfterSend({...intent,hash:'original-hash'});
    if(response==='refused') action.beforeOrAfterSend(null);
    assert.deepEqual(open().read(),other);
    selected.value=eth;
    const replay=open();
    const expected=response==='hash'?{...intent,hash:'original-hash'}:response==='refused'?null:intent;
    assert.deepEqual(replay.read(),expected);
    if(expected) assert.throws(()=>replay.beforeOrAfterSend(intent));
    assert.equal(rows.size,expected?2:1);
  }
});
test('a profile write already in flight retains its original target and clears only its original journal',async()=>{
  const {open,selected}=setup(), action=open(), writes=[];
  action.beforeOrAfterSend(intent);
  const write=action.install('verified-address', async(...args)=>{ await Promise.resolve(); writes.push(args); });
  selected.value=base;
  const other={...intent,chain:8453};
  open().beforeOrAfterSend(other);
  await write;
  action.save(null);
  assert.deepEqual(writes,[['dex.router',{kind:'Dex',id:eth.id},'verified-address',null]]);
  assert.deepEqual(open().read(),other);
  assert.equal(action.read(),null);
});
