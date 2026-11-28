// CI-сборка с подстановкой трёх immutable и сравнением с байткодом получившего оплату BopAmmWallet.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import sha3 from 'js-sha3';
const bin=JSON.parse(readFileSync(process.argv[2]));
const cash=process.argv[3]==='cash';
const card=JSON.parse(readFileSync('ui/src/assets/'+(cash?'bopamm-cash-ethereum-create.json':'bopamm-ethereum-create.json')));
const dir='tests/data/first_run_dex_2026-11-26/';
const read=n=>JSON.parse(gunzipSync(readFileSync(dir+n)));
const facts=read('bopamm_ethereum_deploy.json.gz');
const archived=cash?read('bopamm_ethereum_cash_wallet.json.gz'):facts;
const snap=read('bopamm_ethereum.json.gz').directory;
const history=read('bopamm_ethereum_wallets.json.gz');
const published=read('bopamm_ethereum_published.json.gz');
assert.deepEqual(bin.abi.find(x=>x.type==='constructor').inputs.map(x=>[x.type,x.name]),
  [['address','venue'],['address','expectedPricing'],['address','expectedUsdc']]);
assert.equal(card.sourceBlock,snap.selector.blockHash);
assert.deepEqual(facts.selector,snap.selector);
assert.equal(card.creation,'0x'+bin.creation);
const word=a=>a.slice(2).padStart(64,'0');
const {market,pricing,registry}=card.roles;
const args=[market.address,pricing.address,card.usdc];
const create=Buffer.from(bin.creation+args.map(word).join(''),'hex');
const runtime=Buffer.from(bin.runtime,'hex');
assert.deepEqual(Object.keys(bin.immutableReferences),Object.keys(archived.immutableGroups));
assert.deepEqual(new Set(Object.values(archived.immutableGroups)),new Set(args));
for(const [key,refs] of Object.entries(bin.immutableReferences)) for(const ref of refs){
  assert.equal(ref.length,32);
  Buffer.from(word(archived.immutableGroups[key]),'hex').copy(runtime,ref.start);
}
const digest=b=>createHash('sha256').update(b).digest('hex');
assert.equal(card.creationSha256,digest(create));
assert.equal(card.runtimeSha256,digest(runtime));
assert.equal(card.runtimeKeccak,'0x'+sha3.keccak_256(runtime));
assert.equal(cash?archived.runtime:facts.moduleCode,'0x'+runtime.toString('hex'));
assert.equal(market.hash,snap.roles[market.address].runtime);
assert.equal(snap.roles[market.address].pricing,pricing.address);
assert.equal(pricing.hash,snap.pricing_roles[pricing.address]);
assert.equal(registry.hash,snap.pricing_roles[registry.address]);
for(const [name,r] of Object.entries(card.roles)) {
  assert.equal('0x'+sha3.keccak_256(Buffer.from(facts.roles[name].slice(2),'hex')),r.hash);
}
if(cash) {
  assert.equal(card.moduleRole,'cash_module');
  assert.equal(card.artifactRun,archived.artifactRun);
  const rows=archived.frames.flatMap(f=>f.cases.flatMap(c=>[c.compound,c.direct])).filter(r=>r.outcome==='paid');
  assert.equal(rows.length,15);
  assert(rows.every(r=>r.completed && r.duplicate_reverted && BigInt(r.received)>=BigInt(r.minimum)));
} else {
const tests=published.flatMap(b=>b.frames.flatMap(f=>f.cases));
assert.equal(tests.length,24);
assert.equal(tests.filter(x=>x.after.status==='0x1' && x.after.runtime===card.runtimeKeccak).length,8);
assert(history.every(h=>h.cases.filter(r=>r.after.status==='0x1').every(r=>r.after.runtime===card.runtimeKeccak)));
}
