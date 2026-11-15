// Файл: contracts/evmtest/dmm.mjs
// Что делает: решение 287 — шаг KYBER_DMM (вид 15) ArbExecutor на EVM ethereumjs (Cancun) с пулом-фикстурой
//   fixtures/DmmFixture.sol (строки обмена DMMPool: getTradeInfo, swap с проверкой K по виртуальным резервам):
//   D1) v2: execute USDT → COIN одним шагом 15 по пулу с усилением ×3 и комиссией 0,15 % — прямой вход (transferFrom
//       владелец → пул), владелец получил ровно формулу DMMLibrary.getAmountOut по getTradeInfo пула; minOut + 1 — откат;
//   D2) v2: шаг 15 вторым шагом (вход — баланс исполнителя): цикл USDT →(PancakePair)→ COIN →(пул 15)→ USDT;
//       (пул без усиления — виртуальные резервы = реальные);
//   D3) исполнитель v1 (contracts/out/v1/) — шаг 15 откатывается «kind».
//   Проба на настоящих пулах на цепи — tests/data/kyber_dmm_2026-11-15/exec_sim.json.
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: lib.mjs, world.mjs, fixtures/DmmFixture.sol.
// Настройки: нет.

import { join } from 'node:path';
import { Interface } from 'ethers';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { check, must, view } = L;
const { E18, OWNER, LP, ERC20, v1, v2, world, v2Step, exe, bal, v2Out } = W;
const DMM = L.compile(L.solc08, join(L.here, 'fixtures'), 'DmmFixture.sol', 'DmmFixture', W.opt);
const FEE = 1505309943994589n; // feeInPrecision пула USDT/WBNB 0xec30…6381 на блоке 133680633 (probe.json), над 1e18.
const step = (kind, pool, tin, tout) => [kind, pool, tin, tout, 10000, '0x'];

/// Пул-фикстура USDT/COIN 50 000 : 50 000 от LP, усиление amp. Возвращает адрес.
async function dmmPool(c, usdt, coin, amp) {
  const p = await L.deploy(c, LP, DMM, ['address', 'address', 'uint256', 'uint256'], [usdt, coin, FEE, amp]);
  await must(c, LP, usdt, ERC20.abi, 'transfer', [p, 50_000n * E18]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [p, 50_000n * E18]);
  await must(c, LP, p, DMM.abi, 'sync', []);
  return p;
}

/// Формула DMMLibrary.getAmountOut (L75-L91) по ответу getTradeInfo пула. Возвращает выход.
async function dmmOut(c, p, tin, amt) {
  const i = new Interface(DMM.abi);
  const r = await L.call(c, '0x' + '11'.repeat(20), p, i.encodeFunctionData('getTradeInfo', []));
  const t = [...i.decodeFunctionResult('getTradeInfo', r.ret)].map(BigInt);
  const t0 = await view(c, p, DMM.abi, 'token0', []);
  const [vin, vout] = BigInt(tin) === BigInt(t0) ? [t[2], t[3]] : [t[3], t[2]];
  const wf = (amt * (E18 - t[4])) / E18;
  return (wf * vout) / (vin + wf);
}

/// Сценарии D1–D3. Ошибок нет: провал — check бросает.
export async function dmm() {
  {
    const { c, usdt, coin, ex } = await world(v2);
    const p = await dmmPool(c, usdt, coin, 3n);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const amt = 1000n * E18;
    const exp = await dmmOut(c, p, usdt, amt);
    const bad = await L.call(c, OWNER, ex, exe(usdt, amt, coin, exp + 1n, [step(15, p, usdt, coin)]));
    check(!bad.ok && bad.reason === 'minOut', 'KYBER_DMM: minOut + 1 — откат «minOut»');
    const before = await bal(c, coin, OWNER);
    const r = await L.call(c, OWNER, ex, exe(usdt, amt, coin, exp, [step(15, p, usdt, coin)]));
    check(r.ok, `KYBER_DMM: execute USDT → COIN прошёл (газ ${r.gas})`);
    check((await bal(c, coin, OWNER)) - before === exp, 'KYBER_DMM: владелец получил ровно DMMLibrary.getAmountOut по getTradeInfo');
    check((await bal(c, usdt, ex)) === 0n && (await bal(c, coin, ex)) === 0n, 'KYBER_DMM: на исполнителе ничего не осталось');
  }
  {
    const { c, usdt, coin, pairCU, ex } = await world(v2);
    const p = await dmmPool(c, usdt, coin, 1n);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const amt = 100n * E18;
    const mid = await v2Out(c, pairCU, usdt, amt);
    const exp = await dmmOut(c, p, coin, mid);
    const before = await bal(c, usdt, OWNER);
    const route = [v2Step(pairCU, usdt, coin), step(15, p, coin, usdt)];
    const r = await L.call(c, OWNER, ex, exe(usdt, amt, usdt, 1n, route));
    check(r.ok, `KYBER_DMM вторым шагом (вход — баланс исполнителя, пул без усиления): цикл прошёл (газ ${r.gas})`);
    check((await bal(c, usdt, OWNER)) - (before - amt) === exp, 'KYBER_DMM вторым шагом: выход = формула DMM на выход шага V2');
  }
  {
    const { c, usdt, coin, ex } = await world(v1);
    const p = await dmmPool(c, usdt, coin, 3n);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const r = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 1n, [step(15, p, usdt, coin)]));
    check(!r.ok && r.reason === 'kind', 'исполнитель v1: шаг KYBER_DMM (15) — откат «kind» (вида нет в v1)');
  }
}
