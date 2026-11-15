// Файл: contracts/evmtest/lb.mjs
// Что делает: решение 290 — шаг LB (вид 18) ArbExecutor на EVM ethereumjs (Cancun) с пулом-фикстурой
//   fixtures/LbFixture.sol (порядок обмена LBPair.swap(swapForY, to): вход переводом в пул, выход — `to`; X/Y — порядок
//   создателя, X — больший адрес из пары):
//   B1) v2: execute USDT → COIN одним шагом 18 — прямой вход (transferFrom владелец → пул), владелец получил ровно 99 %;
//       minOut + 1 — откат; на исполнителе пусто;
//   B2) v2: обратная сторона COIN → USDT (swapForY по getTokenX пула, не по адресам);
//   B3) v2: шаг 18 вторым шагом (вход — баланс исполнителя): цикл USDT →(PancakePair)→ COIN →(пул 18)→ USDT;
//   B4) исполнитель v1 (contracts/out/v1/) — шаг 18 откатывается «kind».
//   Проба на настоящих пулах на цепи — tests/data/lb_v21_2026-11-15/exec_state.json.
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: lib.mjs, world.mjs, fixtures/LbFixture.sol.
// Настройки: нет.

import { join } from 'node:path';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { check, must } = L;
const { E18, OWNER, LP, ERC20, v1, v2, world, v2Step, exe, bal, v2Out } = W;
const LB = L.compile(L.solc08, join(L.here, 'fixtures'), 'LbFixture.sol', 'LbFixture', W.opt);
const step = (pool, tin, tout) => [18, pool, tin, tout, 10000, '0x'];

/// Пул-фикстура USDT/COIN 50 000 : 50 000 от LP; X — больший адрес (порядок не по адресам). Возвращает адрес.
async function lbPool(c, usdt, coin) {
  const [x, y] = BigInt(usdt) > BigInt(coin) ? [usdt, coin] : [coin, usdt];
  const p = await L.deploy(c, LP, LB, ['address', 'address'], [x, y]);
  await must(c, LP, usdt, ERC20.abi, 'transfer', [p, 50_000n * E18]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [p, 50_000n * E18]);
  await must(c, LP, p, LB.abi, 'sync', []);
  return p;
}

/// Сценарии B1–B4. Ошибок нет: провал — check бросает.
export async function lb() {
  {
    const { c, usdt, coin, ex } = await world(v2);
    const p = await lbPool(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    await must(c, OWNER, coin, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const amt = 1000n * E18;
    const exp = (amt * 99n) / 100n;
    const bad = await L.call(c, OWNER, ex, exe(usdt, amt, coin, exp + 1n, [step(p, usdt, coin)]));
    check(!bad.ok && bad.reason === 'minOut', 'LB: minOut + 1 — откат «minOut»');
    const before = await bal(c, coin, OWNER);
    const r = await L.call(c, OWNER, ex, exe(usdt, amt, coin, exp, [step(p, usdt, coin)]));
    check(r.ok, `LB: execute USDT → COIN прошёл (газ ${r.gas})`);
    check((await bal(c, coin, OWNER)) - before === exp, 'LB: владелец получил ровно выход пула (прямой вход владелец → пул)');
    check((await bal(c, usdt, ex)) === 0n && (await bal(c, coin, ex)) === 0n, 'LB: на исполнителе ничего не осталось');
    const back = await bal(c, usdt, OWNER);
    const r2 = await L.call(c, OWNER, ex, exe(coin, amt, usdt, (amt * 99n) / 100n, [step(p, coin, usdt)]));
    check(r2.ok && (await bal(c, usdt, OWNER)) - back === (amt * 99n) / 100n, 'LB: обратная сторона COIN → USDT — swapForY по getTokenX пула');
  }
  {
    const { c, usdt, coin, pairCU, ex } = await world(v2);
    const p = await lbPool(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const amt = 100n * E18;
    const mid = await v2Out(c, pairCU, usdt, amt);
    const exp = (mid * 99n) / 100n;
    const before = await bal(c, usdt, OWNER);
    const route = [v2Step(pairCU, usdt, coin), step(p, coin, usdt)];
    const r = await L.call(c, OWNER, ex, exe(usdt, amt, usdt, 1n, route));
    check(r.ok, `LB вторым шагом (вход — баланс исполнителя): цикл прошёл (газ ${r.gas})`);
    check((await bal(c, usdt, OWNER)) - (before - amt) === exp, 'LB вторым шагом: выход = выход пула на выход шага V2');
  }
  {
    const { c, usdt, coin, ex } = await world(v1);
    const p = await lbPool(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const r = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 1n, [step(p, usdt, coin)]));
    check(!r.ok && r.reason === 'kind', 'исполнитель v1: шаг LB (18) — откат «kind» (вида нет в v1)');
  }
}
