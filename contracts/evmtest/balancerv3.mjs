// Файл: contracts/evmtest/balancerv3.mjs
// Что делает: решение 458 (T457-01) — шаг BALANCER_V3 (23) исполнителя ArbExecutor v4 на EVM ethereumjs (Cancun) с
//   ОФИЦИАЛЬНЫМ кодом хранилища Balancer V3 — байткод развёрнутого кода из артефактов balancer/balancer-deployments@b55dc2b8
//   (vendor/balancer-deployments, файлы без изменений, хеши — vendor/SHA256SUMS): v3/tasks/20241204-v3-vault/artifact/
//   {Vault, VaultExtension, VaultAdmin, VaultFactory}.json (solc 0.8.26 via-IR — build-info задачи; развёртывание — как
//   index.ts задачи: VaultFactory(авторизатор, сроки и пределы input.ts) → create(соль, адрес CREATE3, коды)),
//   v3/deprecated/20241205-v3-weighted-pool/artifact/WeightedPoolFactory.json (пул 50/50, комиссия 1 % — как MockWeightedPool
//   задачи), v2/tasks/20210418-authorizer/artifact/Authorizer.json (авторизатор хранилища — input.ts задачи). Ликвидность и котировка —
//   фикстура fixtures/BalancerV3Lp.sol (initialize; quote официального хранилища при tx.origin = 0).
//   B1) обе стороны: владелец получил ровно котировку хранилища (quote → swap) и выход события Swap хранилища, списано
//       ровно amountIn (amountIn события), выход пришёл владельцу переводом от хранилища, на исполнителе пусто,
//       разрешений исполнителя хранилищу нет; minOut + 1 — откат «minOut»;
//   B2) вторым шагом (вход — баланс исполнителя после пары V2) и первым шагом перед парой V2 (выход — исполнителю);
//   B3) защита: посторонний и владелец вне шага зовут balancerV3Callback — «pool»; пул не зарегистрирован в хранилище,
//       токены не пула, «хранилище» без кода хранилища — откат; токен с налогом на входе — откат всего (settle засчитал
//       меньше долга — BalanceNotSettled), владелец при своих; исполнитель v3 (out/v3) — «kind».
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: lib.mjs, world.mjs, vendor/balancer-deployments, fixtures/BalancerV3Lp.sol, fixtures/TaxToken.sol.
// Настройки: нет.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Interface, keccak256 } from 'ethers';
import { hexToBytes, bytesToHex } from '@ethereumjs/util';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { check, must, view, coder } = L;
const { E18, OWNER, LP, OTHER, ERC20, v2, world, v2Step, exe, bal } = W;
const BV3 = 23;
const dep = join(L.here, 'vendor', 'balancer-deployments');
/// Артефакт hardhat из balancer-deployments: { abi, bin (без 0x) }.
const art = (p) => {
  const j = JSON.parse(readFileSync(join(dep, p), 'utf8'));
  return { abi: j.abi, bin: j.bytecode.slice(2), deployed: j.deployedBytecode.slice(2) };
};
const VAULT = art('v3/tasks/20241204-v3-vault/artifact/Vault.json');
const VEXT = art('v3/tasks/20241204-v3-vault/artifact/VaultExtension.json');
const VADM = art('v3/tasks/20241204-v3-vault/artifact/VaultAdmin.json');
const VFAC = art('v3/tasks/20241204-v3-vault/artifact/VaultFactory.json');
const WPF = art('v3/deprecated/20241205-v3-weighted-pool/artifact/WeightedPoolFactory.json');
const AUTH = art('v2/tasks/20210418-authorizer/artifact/Authorizer.json');
const LPF = L.compile(L.solc08, join(L.here, 'fixtures'), 'BalancerV3Lp.sol', 'BalancerV3Lp', W.opt);
const TAX = L.compile(L.solc08, join(L.here, 'fixtures'), 'TaxToken.sol', 'TaxToken', W.opt);
// ABI хранилища целиком: Vault + VaultExtension + VaultAdmin (вызовы расширения идут через fallback Vault), без повторов.
const seen = new Set();
const VABI = [...VAULT.abi, ...VEXT.abi, ...VADM.abi].filter((f) => {
  if (f.type === 'constructor' || f.type === 'fallback' || f.type === 'receive') return false;
  const k = `${f.type}:${f.name}:${(f.inputs ?? []).map((i) => i.type).join(',')}`;
  if (seen.has(k)) return false;
  seen.add(k);
  return true;
});
const VI = new Interface(VABI);
// input.ts задачи 20241204-v3-vault: MONTH = 30 дней (helpers/time.ts L48), пределы 1e6 / 1e4, соль задачи.
const MONTH = 30n * 86400n;
const SALT = '0x3877188e9e5da25b11fdb7f5e8d4fdddce2d22707ba04878a8e14700dd46fa82';
const ZERO = '0x' + '00'.repeat(20);
const sort = (a, b) => (BigInt(a) < BigInt(b) ? [a, b] : [b, a]);
const step = (vault, pool, tin, tout, bps = 10000) => [BV3, vault, tin, tout, bps, coder.encode(['address'], [pool])];

/// Хранилище V3 как в index.ts задачи: авторизатор (аргумент Vault) → VaultFactory (create — onlyOwner, владелец LP) → create. Возвращает адрес Vault.
async function vaultOf(c) {
  const auth = await L.deploy(c, LP, AUTH, ['address'], [LP]);
  const h = (a) => keccak256('0x' + a.bin);
  const fac = await L.deploy(c, LP, VFAC, ['address', 'uint32', 'uint32', 'uint256', 'uint256', 'bytes32', 'bytes32', 'bytes32'],
    [auth, 4n * 12n * MONTH, 6n * MONTH, 10n ** 6n, 10n ** 4n, h(VAULT), h(VEXT), h(VADM)]);
  const target = await view(c, fac, VFAC.abi, 'getDeploymentAddress', [SALT]);
  await must(c, LP, fac, VFAC.abi, 'create', [SALT, target, '0x' + VAULT.bin, '0x' + VEXT.bin, '0x' + VADM.bin]);
  return target;
}

/// Пул 50/50 токенов a/b (комиссия 1 %) от официальной фабрики 20241205 с первой ликвидностью la/lb. Возвращает адрес.
async function poolOf(c, vault, wpf, lp, a, b, amt, n) {
  const [t0, t1] = sort(a, b);
  const tok = [t0, t1].map((t) => [t, 0, ZERO, false]);
  const r = await must(c, LP, wpf, WPF.abi, 'create', [`T${n}`, `T${n}`, tok, [E18 / 2n, E18 / 2n], [ZERO, ZERO, ZERO],
    E18 / 100n, ZERO, false, false, '0x' + n.toString(16).padStart(64, '0')]);
  const pool = new Interface(WPF.abi).decodeFunctionResult('create', r.ret)[0];
  await must(c, LP, lp, LPF.abi, 'init', [vault, pool, [t0, t1], [amt, amt]]);
  return pool;
}

/// Мир: world.mjs (исполнитель art) + хранилище V3, фабрика пулов, пул USDT/COIN, разрешения владельца исполнителю.
async function env(art0 = v2) {
  const w = await world(art0);
  const { c, usdt, coin, ex } = w;
  const vault = await vaultOf(c);
  const wpf = await L.deploy(c, LP, WPF, ['address', 'uint32', 'string', 'string'], [vault, 4n * 12n * MONTH, 'f', 'p']);
  const lp = await L.deploy(c, LP, LPF);
  for (const t of [usdt, coin]) await must(c, LP, t, ERC20.abi, 'transfer', [lp, 100_000n * E18]);
  const pool = await poolOf(c, vault, wpf, lp, usdt, coin, 10_000n * E18, 1);
  for (const t of [usdt, coin]) await must(c, OWNER, t, ERC20.abi, 'approve', [ex, 10n ** 30n]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [OWNER, 10_000n * E18]);
  return { ...w, vault, wpf, lp, pool };
}

/// Котировка точного входа официальным хранилищем (quote при tx.origin = 0; состояние откатывается). Возвращает выход.
async function quote(e, tin, tout, amt) {
  const { c, vault, lp, pool } = e;
  await c.sm.checkpoint();
  const r = await c.evm.runCall({
    caller: L.addr(OTHER), origin: L.addr(ZERO), to: L.addr(lp), gasLimit: 30_000_000n,
    data: hexToBytes(new Interface(LPF.abi).encodeFunctionData('quoteIn', [vault, pool, tin, tout, amt])),
  });
  await c.sm.revert();
  if (r.execResult.exceptionError) throw new Error(`quote: откат ${bytesToHex(r.execResult.returnValue)}`);
  return BigInt(coder.decode(['uint256'], bytesToHex(r.execResult.returnValue))[0]);
}

/// События Swap хранилища в итоге вызова: [{ amountIn, amountOut }].
/// Имя ошибки хранилища (custom error) по данным отката; не распознана — null.
const err = (r) => { try { return VI.parseError(r.ret)?.name ?? null; } catch { return null; } };

const swaps = (r, vault) => r.logs
  .filter((l) => l.address.toLowerCase() === vault.toLowerCase() && l.topics[0] === VI.getEvent('Swap').topicHash)
  .map((l) => VI.decodeEventLog('Swap', l.data, l.topics));

/// Переводы токена t от from к to в итоге вызова.
const transfers = (r, t, from, to) => {
  const T = new Interface(ERC20.abi).getEvent('Transfer').topicHash;
  return r.logs.filter((l) => l.address.toLowerCase() === t.toLowerCase() && l.topics[0] === T
    && BigInt(l.topics[1]) === BigInt(from) && BigInt(l.topics[2]) === BigInt(to));
};

/// Сценарии B1–B3. Возвращает запись для тестов бота (шаги B1, прошедшие в EVM). Провал — check бросает.
export async function balancerv3() {
  const record = { note: 'EVM ethereumjs Cancun, contracts/out v4, официальный Vault V3 (balancer-deployments@b55dc2b8) — contracts/evmtest/balancerv3.mjs', cases: [] };
  console.log(`официальный Balancer V3: Vault ${VAULT.deployed.length / 2} байт, VaultExtension ${VEXT.deployed.length / 2}, VaultAdmin ${VADM.deployed.length / 2}`);
  // B1.
  {
    const e = await env();
    const { c, ex, usdt, coin, vault, pool } = e;
    check((await view(c, vault, VABI, 'isPoolRegistered', [pool])) && (await view(c, vault, VABI, 'isPoolInitialized', [pool])),
      'BALANCER_V3: пул зарегистрирован и инициализирован в официальном хранилище');
    const amt = 100n * E18;
    for (const [tin, tout, dir] of [[usdt, coin, 'USDT → COIN'], [coin, usdt, 'COIN → USDT']]) {
      const q = await quote(e, tin, tout, amt);
      const s = step(vault, pool, tin, tout);
      const bad = await L.call(c, OWNER, ex, exe(tin, amt, tout, q + 1n, [s]));
      check(!bad.ok && bad.reason === 'minOut', `BALANCER_V3 ${dir}: minOut = котировка + 1 — откат «minOut»`);
      const ib = await bal(c, tin, OWNER);
      const ob = await bal(c, tout, OWNER);
      const r = await L.call(c, OWNER, ex, exe(tin, amt, tout, q, [s]));
      check(r.ok, `BALANCER_V3 ${dir}: обмен прошёл (газ ${r.gas})`);
      const got = (await bal(c, tout, OWNER)) - ob;
      const ev = swaps(r, vault);
      check(ev.length === 1 && got === q && BigInt(ev[0].amountOut) === q && q > 0n,
        `BALANCER_V3 ${dir}: владелец получил ровно котировку хранилища и выход события Swap (${got})`);
      check(ib - (await bal(c, tin, OWNER)) === amt && BigInt(ev[0].amountIn) === amt,
        `BALANCER_V3 ${dir}: списано ровно amountIn, хранилище взяло ровно его`);
      check(transfers(r, tout, vault, OWNER).length === 1, `BALANCER_V3 ${dir}: выход — переводом хранилище → владелец (sendTo получателю шага)`);
      check((await bal(c, tin, ex)) === 0n && (await bal(c, tout, ex)) === 0n
        && BigInt(await view(c, tin, ERC20.abi, 'allowance', [ex, vault])) === 0n,
        `BALANCER_V3 ${dir}: на исполнителе пусто, разрешений хранилищу нет`);
      record.cases.push({ kind: BV3, vault, pool, token_in: tin, token_out: tout, data: s[5], amount_in: amt.toString(), got: got.toString(), gas: r.gas.toString() });
    }
  }
  // B2: вторым шагом и первым шагом маршрута из двух.
  {
    const e = await env();
    const { c, ex, usdt, coin, w, pair, pairCW, vault, pool } = e;
    const ib = await bal(c, w, OWNER);
    const r1 = await L.call(c, OWNER, ex, exe(usdt, 100n * E18, w, 1n, [step(vault, pool, usdt, coin), v2Step(pairCW, coin, w)]));
    const mid = swaps(r1, vault);
    check(r1.ok && mid.length === 1 && transfers(r1, coin, vault, ex).length === 1 && (await bal(c, w, OWNER)) > ib,
      'BALANCER_V3 первым шагом: выход — исполнителю (следующий шаг), затем пара V2 → WBNB владельцу');
    check((await bal(c, coin, ex)) === 0n && (await bal(c, usdt, ex)) === 0n, 'BALANCER_V3 первым шагом: на исполнителе пусто');
    const cb = await bal(c, coin, OWNER);
    const r2 = await L.call(c, OWNER, ex, exe(usdt, 100n * E18, coin, 1n, [v2Step(pair, usdt, w), step(vault, pool, w, coin)]));
    check(!r2.ok && err(r2) === 'TokenNotRegistered', `BALANCER_V3 с токеном не пула (WBNB → COIN в пуле USDT/COIN) — откат ${err(r2)}`);
    const ub = await bal(c, usdt, OWNER);
    const amt = 20n * E18;
    const r4 = await L.call(c, OWNER, ex, exe(usdt, amt, coin, 1n, [step(vault, pool, usdt, coin, 5000), step(vault, pool, usdt, coin)]));
    const ev = swaps(r4, vault);
    check(r4.ok && ev.length === 2 && BigInt(ev[0].amountIn) === amt / 2n && BigInt(ev[1].amountIn) === amt / 2n,
      'BALANCER_V3 двумя долями (50 % + остаток): второй шаг берёт вход с баланса исполнителя');
    check((await bal(c, coin, OWNER)) > cb && (await bal(c, usdt, ex)) === 0n && ub - (await bal(c, usdt, OWNER)) === amt,
      'BALANCER_V3 двумя долями: выход владельцу, на исполнителе пусто');
  }
  // B3: защита.
  {
    const e = await env();
    const { c, ex, usdt, coin, pair, vault, pool, wpf, lp } = e;
    const X = new Interface(v2.abi);
    const cb = coder.encode(['address', 'address', 'address', 'uint256', 'address'], [pool, usdt, coin, E18, OTHER]);
    for (const [who, name] of [[OTHER, 'посторонний'], [OWNER, 'владелец вне шага'], [vault, 'хранилище вне шага']]) {
      const r = await L.call(c, who, ex, X.encodeFunctionData('balancerV3Callback', [cb]));
      check(!r.ok && r.reason === 'pool', `${name} зовёт balancerV3Callback — откат «pool»`);
    }
    const np = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 0n, [step(vault, pair, usdt, coin)]));
    check(!np.ok && err(np) === 'PoolNotInitialized', `пул не зарегистрирован в хранилище (адрес пары V2) — откат Vault.swap ${err(np)}`);
    const nv = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 0n, [step(pair, pool, usdt, coin)]));
    // У PancakePair нет unlock и нет fallback — откат без данных.
    check(!nv.ok && nv.ret === '0x', '«хранилище» шага — не хранилище (пара V2, нет unlock) — откат без данных');
    // Токен с налогом 1 % (fixtures/TaxToken.sol) в пуле: settle засчитает min(подсказка, прирост) — долг не погашен.
    const tax = await L.deploy(c, LP, TAX, ['uint256'], [10n ** 30n]);
    await must(c, LP, tax, TAX.abi, 'setExempt', [lp, true]);
    await must(c, LP, tax, TAX.abi, 'transfer', [lp, 100_000n * E18]);
    await must(c, LP, tax, TAX.abi, 'transfer', [OWNER, 1_000n * E18]);
    const tp = await poolOf(c, vault, wpf, lp, tax, coin, 10_000n * E18, 2);
    await must(c, OWNER, tax, TAX.abi, 'approve', [ex, 10n ** 30n]);
    const tb = BigInt(await view(c, tax, TAX.abi, 'balanceOf', [OWNER]));
    const tr = await L.call(c, OWNER, ex, exe(tax, 10n * E18, coin, 0n, [step(vault, tp, tax, coin)]));
    check(!tr.ok && err(tr) === 'BalanceNotSettled' && BigInt(await view(c, tax, TAX.abi, 'balanceOf', [OWNER])) === tb,
      `токен с налогом на входе шага BALANCER_V3 — откат всего (${err(tr)}), владелец при своих`);
  }
  {
    const e = await env(W.v3f);
    const r = await L.call(e.c, OWNER, e.ex, exe(e.usdt, E18, e.coin, 0n, [step(e.vault, e.pool, e.usdt, e.coin)]));
    check(!r.ok && r.reason === 'kind', 'исполнитель v3 (out/v3): шаг BALANCER_V3 (23) — откат «kind»');
  }
  return record;
}
